use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit,
    aead::{Aead, Payload},
};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use snow::{Builder, HandshakeState, TransportState, params::NoiseParams};
use transfer_protocol::{
    FILE_SALT_LEN, MAX_FILE_NAME_LEN, RECORD_CHUNK, SESSION_ID_LEN, SHA256_LEN,
    TRANSFER_PROTOCOL_VERSION,
};
use zeroize::{Zeroize, Zeroizing};

const NOISE_PATTERN: &str = "Noise_NNpsk0_25519_ChaChaPoly_SHA256";
const SESSION_BOOTSTRAP_CONTEXT: &[u8] = b"pico-transfer-unattended-v2";
const FILE_KEY_INFO: &[u8] = b"pico-transfer-v2/file-key";
const SESSION_MASTER_LEN: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("cryptographic random source unavailable")]
    Random,
    #[error("invalid session bootstrap secret")]
    BootstrapSecret,
    #[error("noise protocol failure")]
    Noise,
    #[error("key derivation failure")]
    KeyDerivation,
    #[error("authentication failed")]
    Authentication,
    #[error("invalid plaintext")]
    InvalidPlaintext,
}

pub type SessionMaster = Zeroizing<[u8; SESSION_MASTER_LEN]>;

pub fn fill_random(out: &mut [u8]) -> Result<(), CryptoError> {
    getrandom::fill(out).map_err(|_| CryptoError::Random)
}

pub fn generate_bootstrap_secret() -> Result<Zeroizing<String>, CryptoError> {
    let mut bytes = Zeroizing::new([0u8; 16]);
    fill_random(bytes.as_mut())?;
    let mut out = Zeroizing::new(String::with_capacity(32));
    for byte in bytes.iter() {
        use core::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    Ok(out)
}

pub fn new_session_master() -> Result<SessionMaster, CryptoError> {
    let mut master = Zeroizing::new([0; SESSION_MASTER_LEN]);
    fill_random(master.as_mut())?;
    Ok(master)
}

pub fn derive_session_psk(
    code: &str,
    session_id: &[u8; SESSION_ID_LEN],
) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    if code.len() != 32 || !code.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CryptoError::BootstrapSecret);
    }
    let mut code_bytes = Zeroizing::new([0u8; 16]);
    for (index, chunk) in code.as_bytes().chunks_exact(2).enumerate() {
        let text = core::str::from_utf8(chunk).map_err(|_| CryptoError::BootstrapSecret)?;
        code_bytes[index] =
            u8::from_str_radix(text, 16).map_err(|_| CryptoError::BootstrapSecret)?;
    }
    let mut hasher = Sha256::new();
    hasher.update(SESSION_BOOTSTRAP_CONTEXT);
    hasher.update(session_id);
    hasher.update(code_bytes.as_slice());
    Ok(Zeroizing::new(hasher.finalize().into()))
}

pub struct BrowserHandshake {
    state: HandshakeState,
}

impl BrowserHandshake {
    pub fn start(psk: &[u8; 32]) -> Result<(Self, Vec<u8>), CryptoError> {
        let params: NoiseParams = NOISE_PATTERN.parse().map_err(|_| CryptoError::Noise)?;
        let mut state = Builder::new(params)
            .psk(0, psk)
            .map_err(|_| CryptoError::Noise)?
            .build_initiator()
            .map_err(|_| CryptoError::Noise)?;
        let mut message = vec![0; 128];
        let len = state
            .write_message(&[], &mut message)
            .map_err(|_| CryptoError::Noise)?;
        message.truncate(len);
        Ok((Self { state }, message))
    }

    pub fn finish(mut self, response: &[u8]) -> Result<BrowserTransport, CryptoError> {
        let mut payload = [0; 32];
        let len = self
            .state
            .read_message(response, &mut payload)
            .map_err(|_| CryptoError::Noise)?;
        if len != 0 || !self.state.is_handshake_finished() {
            return Err(CryptoError::Noise);
        }
        Ok(BrowserTransport {
            state: self
                .state
                .into_transport_mode()
                .map_err(|_| CryptoError::Noise)?,
        })
    }
}

pub struct HostHandshake {
    state: HandshakeState,
}

impl HostHandshake {
    pub fn new(psk: &[u8; 32]) -> Result<Self, CryptoError> {
        let params: NoiseParams = NOISE_PATTERN.parse().map_err(|_| CryptoError::Noise)?;
        let state = Builder::new(params)
            .psk(0, psk)
            .map_err(|_| CryptoError::Noise)?
            .build_responder()
            .map_err(|_| CryptoError::Noise)?;
        Ok(Self { state })
    }

    pub fn respond(mut self, request: &[u8]) -> Result<(HostTransport, Vec<u8>), CryptoError> {
        let mut payload = [0; 32];
        let len = self
            .state
            .read_message(request, &mut payload)
            .map_err(|_| CryptoError::Noise)?;
        if len != 0 {
            return Err(CryptoError::Noise);
        }
        let mut response = vec![0; 128];
        let response_len = self
            .state
            .write_message(&[], &mut response)
            .map_err(|_| CryptoError::Noise)?;
        response.truncate(response_len);
        if !self.state.is_handshake_finished() {
            return Err(CryptoError::Noise);
        }
        Ok((
            HostTransport {
                state: self
                    .state
                    .into_transport_mode()
                    .map_err(|_| CryptoError::Noise)?,
            },
            response,
        ))
    }
}

pub struct BrowserTransport {
    state: TransportState,
}

impl BrowserTransport {
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        noise_seal(&mut self.state, plaintext)
    }

    pub fn open(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        noise_open(&mut self.state, ciphertext)
    }
}

pub struct HostTransport {
    state: TransportState,
}

impl HostTransport {
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        noise_seal(&mut self.state, plaintext)
    }

    pub fn open(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        noise_open(&mut self.state, ciphertext)
    }
}

fn noise_seal(state: &mut TransportState, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let mut out = vec![0; plaintext.len() + 16];
    let len = state
        .write_message(plaintext, &mut out)
        .map_err(|_| CryptoError::Noise)?;
    out.truncate(len);
    Ok(out)
}

fn noise_open(state: &mut TransportState, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let mut out = vec![0; ciphertext.len()];
    let len = state
        .read_message(ciphertext, &mut out)
        .map_err(|_| CryptoError::Authentication)?;
    out.truncate(len);
    Ok(out)
}

pub struct FileCipher {
    cipher: ChaCha20Poly1305,
    aad_prefix: Vec<u8>,
}

impl FileCipher {
    pub fn new(
        session_master: &[u8; 32],
        session_id: &[u8; SESSION_ID_LEN],
        transfer_id: u64,
        file_salt: &[u8; FILE_SALT_LEN],
        chunk_size: u16,
        chunk_count: u32,
    ) -> Result<Self, CryptoError> {
        let hkdf = Hkdf::<Sha256>::new(Some(file_salt), session_master);
        let mut info = Vec::with_capacity(FILE_KEY_INFO.len() + SESSION_ID_LEN + 8);
        info.extend_from_slice(FILE_KEY_INFO);
        info.extend_from_slice(session_id);
        info.extend_from_slice(&transfer_id.to_le_bytes());
        let mut key = Zeroizing::new([0u8; 32]);
        hkdf.expand(&info, key.as_mut())
            .map_err(|_| CryptoError::KeyDerivation)?;
        let cipher = ChaCha20Poly1305::new_from_slice(key.as_slice())
            .map_err(|_| CryptoError::KeyDerivation)?;

        let mut aad_prefix = Vec::with_capacity(2 + SESSION_ID_LEN + 8 + FILE_SALT_LEN + 2 + 4);
        aad_prefix.extend_from_slice(&TRANSFER_PROTOCOL_VERSION.to_le_bytes());
        aad_prefix.extend_from_slice(session_id);
        aad_prefix.extend_from_slice(&transfer_id.to_le_bytes());
        aad_prefix.extend_from_slice(file_salt);
        aad_prefix.extend_from_slice(&chunk_size.to_le_bytes());
        aad_prefix.extend_from_slice(&chunk_count.to_le_bytes());
        Ok(Self { cipher, aad_prefix })
    }

    pub fn seal(
        &self,
        record_type: u8,
        index: u32,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        let nonce = record_nonce(record_type, index);
        let aad = self.aad(record_type, index);
        self.cipher
            .encrypt(
                (&nonce).into(),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| CryptoError::Authentication)
    }

    pub fn open(
        &self,
        record_type: u8,
        index: u32,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        let nonce = record_nonce(record_type, index);
        let aad = self.aad(record_type, index);
        self.cipher
            .decrypt(
                (&nonce).into(),
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| CryptoError::Authentication)
    }

    pub fn seal_chunk(&self, index: u32, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        self.seal(RECORD_CHUNK, index, plaintext)
    }

    pub fn open_chunk(&self, index: u32, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        self.open(RECORD_CHUNK, index, ciphertext)
    }

    fn aad(&self, record_type: u8, index: u32) -> Vec<u8> {
        let mut aad = Vec::with_capacity(self.aad_prefix.len() + 5);
        aad.extend_from_slice(&self.aad_prefix);
        aad.push(record_type);
        aad.extend_from_slice(&index.to_le_bytes());
        aad
    }
}

fn record_nonce(record_type: u8, index: u32) -> [u8; 12] {
    let mut nonce = [0; 12];
    nonce[0] = record_type;
    nonce[8..].copy_from_slice(&index.to_be_bytes());
    nonce
}

pub fn encode_manifest(file_name: &str, total_size: u64) -> Result<Vec<u8>, CryptoError> {
    let name = file_name.as_bytes();
    if name.is_empty() || name.len() > MAX_FILE_NAME_LEN {
        return Err(CryptoError::InvalidPlaintext);
    }
    let name_len = u16::try_from(name.len()).map_err(|_| CryptoError::InvalidPlaintext)?;
    let mut out = Vec::with_capacity(1 + 8 + 2 + name.len());
    out.push(1);
    out.extend_from_slice(&total_size.to_le_bytes());
    out.extend_from_slice(&name_len.to_le_bytes());
    out.extend_from_slice(name);
    Ok(out)
}

pub fn decode_manifest(input: &[u8]) -> Result<(u64, String), CryptoError> {
    if input.len() < 11 || input[0] != 1 {
        return Err(CryptoError::InvalidPlaintext);
    }
    let total_size = u64::from_le_bytes(
        input[1..9]
            .try_into()
            .map_err(|_| CryptoError::InvalidPlaintext)?,
    );
    let name_len = u16::from_le_bytes(
        input[9..11]
            .try_into()
            .map_err(|_| CryptoError::InvalidPlaintext)?,
    ) as usize;
    if name_len == 0 || name_len > MAX_FILE_NAME_LEN || input.len() != 11 + name_len {
        return Err(CryptoError::InvalidPlaintext);
    }
    let name = core::str::from_utf8(&input[11..])
        .map_err(|_| CryptoError::InvalidPlaintext)?
        .to_string();
    Ok((total_size, name))
}

pub fn encode_close(total_size: u64, chunk_count: u32, sha256: &[u8; SHA256_LEN]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + 4 + SHA256_LEN);
    out.extend_from_slice(&total_size.to_le_bytes());
    out.extend_from_slice(&chunk_count.to_le_bytes());
    out.extend_from_slice(sha256);
    out
}

pub fn decode_close(input: &[u8]) -> Result<(u64, u32, [u8; SHA256_LEN]), CryptoError> {
    if input.len() != 8 + 4 + SHA256_LEN {
        return Err(CryptoError::InvalidPlaintext);
    }
    let total_size = u64::from_le_bytes(
        input[0..8]
            .try_into()
            .map_err(|_| CryptoError::InvalidPlaintext)?,
    );
    let chunk_count = u32::from_le_bytes(
        input[8..12]
            .try_into()
            .map_err(|_| CryptoError::InvalidPlaintext)?,
    );
    let mut sha256 = [0; SHA256_LEN];
    sha256.copy_from_slice(&input[12..]);
    Ok((total_size, chunk_count, sha256))
}

pub fn encode_receipt(transfer_id: u64, sha256: &[u8; SHA256_LEN]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 8 + SHA256_LEN);
    out.push(transfer_protocol::CONTROL_TRANSFER_RECEIPT);
    out.extend_from_slice(&transfer_id.to_le_bytes());
    out.extend_from_slice(sha256);
    out
}

impl Drop for FileCipher {
    fn drop(&mut self) {
        self.aad_prefix.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transfer_protocol::{RECORD_CLOSE, RECORD_MANIFEST};

    #[test]
    fn noise_session_transports_browser_session_master() {
        let psk = [9; 32];
        let (browser, first) = BrowserHandshake::start(&psk).unwrap();
        let host = HostHandshake::new(&psk).unwrap();
        let (mut host, second) = host.respond(&first).unwrap();
        let mut browser = browser.finish(&second).unwrap();
        let sealed = browser.seal(&[7; 32]).unwrap();
        assert_eq!(host.open(&sealed).unwrap(), [7; 32]);
        let confirmation = host.seal(b"ready").unwrap();
        assert_eq!(browser.open(&confirmation).unwrap(), b"ready");
    }

    #[test]
    fn records_are_domain_separated_and_authenticated() {
        let cipher = FileCipher::new(&[1; 32], &[2; 16], 3, &[4; 32], 2002, 1).unwrap();
        let sealed = cipher.seal(RECORD_MANIFEST, 0, b"payload").unwrap();
        assert_eq!(
            cipher.open(RECORD_MANIFEST, 0, &sealed).unwrap(),
            b"payload"
        );
        assert!(cipher.open(RECORD_CLOSE, 0, &sealed).is_err());
        assert!(cipher.open(RECORD_MANIFEST, 1, &sealed).is_err());
    }

    #[test]
    fn manifest_and_close_roundtrip() {
        let manifest = encode_manifest("hello.bin", 44).unwrap();
        assert_eq!(
            decode_manifest(&manifest).unwrap(),
            (44, "hello.bin".into())
        );
        let close = encode_close(44, 2, &[8; 32]);
        assert_eq!(decode_close(&close).unwrap(), (44, 2, [8; 32]));
    }
}
