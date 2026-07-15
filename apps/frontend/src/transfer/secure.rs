use std::collections::HashMap;

use transfer_crypto::{
    BrowserHandshake, BrowserTransport, FileCipher, SessionMaster, decode_close, decode_manifest,
    derive_session_psk, encode_receipt, new_session_master,
};
use transfer_protocol::{
    CONTROL_SET_DEFAULT_PATH, CONTROL_START_DEFAULT_TRANSFER, CONTROL_START_TRANSFER,
    MAX_TRANSFER_PATH_LEN, RECORD_CLOSE, RECORD_MANIFEST, SESSION_ENVELOPE_HEADER_LEN,
    SESSION_ID_LEN, SESSION_KIND_HANDSHAKE, SESSION_KIND_READY, SESSION_KIND_REQUEST,
    SESSION_KIND_TRANSPORT, decode_secure_chunk, decode_secure_close, decode_secure_open,
    decode_session_envelope, encode_session_envelope,
};
use zeroize::Zeroizing;

const SESSION_CONFIRMATION: &[u8] = b"pico-transfer-ready-v1";
const MAX_SESSION_ENVELOPE: usize = transfer_protocol::MAX_SECURE_SESSION_FRAME;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecureSessionView {
    Idle,
    Negotiating,
    Handshaking,
    Established,
    Failed,
}

impl SecureSessionView {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Not connected",
            Self::Negotiating => "Negotiating with host",
            Self::Handshaking => "Establishing encryption",
            Self::Established => "Encrypted session ready",
            Self::Failed => "Connection failed",
        }
    }

    pub fn established(self) -> bool {
        self == Self::Established
    }
}

enum State {
    Idle,
    Requested,
    Handshaking {
        session_id: [u8; SESSION_ID_LEN],
        handshake: Box<BrowserHandshake>,
    },
    AwaitingConfirmation {
        session_id: [u8; SESSION_ID_LEN],
        transport: BrowserTransport,
        master: SessionMaster,
    },
    Established {
        session_id: [u8; SESSION_ID_LEN],
        transport: BrowserTransport,
        master: SessionMaster,
        files: HashMap<u64, FileCipher>,
    },
    Failed,
}

pub struct SecureSession {
    state: State,
}

pub struct SecureOpenData {
    pub transfer_id: u64,
    pub file_name: String,
    pub total_size: u64,
    pub chunk_size: u16,
    pub chunk_count: u32,
}

pub struct SecureCloseData {
    pub transfer_id: u64,
    pub total_size: u64,
    pub chunk_count: u32,
    pub sha256: [u8; 32],
}

impl SecureSession {
    pub fn new() -> Self {
        Self { state: State::Idle }
    }

    pub fn view(&self) -> SecureSessionView {
        match self.state {
            State::Idle => SecureSessionView::Idle,
            State::Requested => SecureSessionView::Negotiating,
            State::Handshaking { .. } | State::AwaitingConfirmation { .. } => {
                SecureSessionView::Handshaking
            }
            State::Established { .. } => SecureSessionView::Established,
            State::Failed => SecureSessionView::Failed,
        }
    }

    pub fn reset(&mut self) {
        self.state = State::Idle;
    }

    pub fn request_session(&mut self) -> Result<Vec<u8>, String> {
        let payload = encode_session(SESSION_KIND_REQUEST, &[0; SESSION_ID_LEN], &[])?;
        self.state = State::Requested;
        Ok(payload)
    }

    pub fn handle_session(&mut self, payload: &[u8]) -> Result<Option<Vec<u8>>, String> {
        let envelope = decode_session_envelope(payload).map_err(protocol_error)?;
        let state = std::mem::replace(&mut self.state, State::Failed);
        match (state, envelope.kind) {
            (State::Requested, SESSION_KIND_READY) => {
                let code = core::str::from_utf8(envelope.body)
                    .map_err(|_| "host returned an invalid session bootstrap secret")?;
                let psk = derive_session_psk(code, &envelope.session_id).map_err(crypto_error)?;
                let (handshake, body) = BrowserHandshake::start(&psk).map_err(crypto_error)?;
                let outbound = encode_session(SESSION_KIND_HANDSHAKE, &envelope.session_id, &body)?;
                self.state = State::Handshaking {
                    session_id: envelope.session_id,
                    handshake: Box::new(handshake),
                };
                Ok(Some(outbound))
            }
            (
                State::Handshaking {
                    session_id,
                    handshake,
                },
                SESSION_KIND_HANDSHAKE,
            ) if envelope.session_id == session_id => {
                let mut transport = (*handshake).finish(envelope.body).map_err(crypto_error)?;
                let master = new_session_master().map_err(crypto_error)?;
                let sealed_master = transport.seal(master.as_ref()).map_err(crypto_error)?;
                let outbound = encode_session(SESSION_KIND_TRANSPORT, &session_id, &sealed_master)?;
                self.state = State::AwaitingConfirmation {
                    session_id,
                    transport,
                    master,
                };
                Ok(Some(outbound))
            }
            (
                State::AwaitingConfirmation {
                    session_id,
                    mut transport,
                    master,
                },
                SESSION_KIND_TRANSPORT,
            ) if envelope.session_id == session_id => {
                let confirmation = transport.open(envelope.body).map_err(crypto_error)?;
                if confirmation != SESSION_CONFIRMATION {
                    return Err("host returned an invalid secure-session confirmation".into());
                }
                self.state = State::Established {
                    session_id,
                    transport,
                    master,
                    files: HashMap::new(),
                };
                Ok(None)
            }
            (previous, _) => {
                self.state = previous;
                Err("unexpected secure-session message".into())
            }
        }
    }

    pub fn seal_start(&mut self, path: &str) -> Result<Vec<u8>, String> {
        self.seal_path_control(CONTROL_START_TRANSFER, path)
    }

    pub fn seal_default_path(&mut self, path: &str) -> Result<Vec<u8>, String> {
        self.seal_path_control(CONTROL_SET_DEFAULT_PATH, path)
    }

    pub fn seal_start_default(&mut self) -> Result<Vec<u8>, String> {
        self.seal_control(&[CONTROL_START_DEFAULT_TRANSFER])
    }

    fn seal_path_control(&mut self, kind: u8, path: &str) -> Result<Vec<u8>, String> {
        if path.is_empty() || path.len() > MAX_TRANSFER_PATH_LEN {
            return Err(format!(
                "path must contain 1 to {MAX_TRANSFER_PATH_LEN} UTF-8 bytes"
            ));
        }
        let mut plaintext = Zeroizing::new(Vec::with_capacity(path.len() + 1));
        plaintext.push(kind);
        plaintext.extend_from_slice(path.as_bytes());
        self.seal_control(&plaintext)
    }

    pub fn decrypt_open(&mut self, payload: &[u8]) -> Result<SecureOpenData, String> {
        let open = decode_secure_open(payload).map_err(protocol_error)?;
        let State::Established {
            session_id,
            master,
            files,
            ..
        } = &mut self.state
        else {
            return Err("encrypted file received without an authenticated session".into());
        };
        if open.session_id != *session_id || files.contains_key(&open.transfer_id) {
            return Err("file open does not belong to the active session".into());
        }
        let cipher = FileCipher::new(
            master,
            session_id,
            open.transfer_id,
            &open.file_salt,
            open.chunk_size,
            open.chunk_count,
        )
        .map_err(crypto_error)?;
        let manifest = Zeroizing::new(
            cipher
                .open(RECORD_MANIFEST, 0, open.ciphertext)
                .map_err(crypto_error)?,
        );
        let (total_size, file_name) = decode_manifest(&manifest).map_err(crypto_error)?;
        files.insert(open.transfer_id, cipher);
        Ok(SecureOpenData {
            transfer_id: open.transfer_id,
            file_name,
            total_size,
            chunk_size: open.chunk_size,
            chunk_count: open.chunk_count,
        })
    }

    pub fn decrypt_chunk(&self, payload: &[u8]) -> Result<(u64, u32, Zeroizing<Vec<u8>>), String> {
        let chunk = decode_secure_chunk(payload).map_err(protocol_error)?;
        let State::Established {
            session_id, files, ..
        } = &self.state
        else {
            return Err("encrypted file received without an authenticated session".into());
        };
        if chunk.session_id != *session_id {
            return Err("file chunk does not belong to the active session".into());
        }
        let cipher = files
            .get(&chunk.transfer_id)
            .ok_or_else(|| "chunk received before its authenticated manifest".to_string())?;
        let plaintext = cipher
            .open_chunk(chunk.chunk_index, chunk.ciphertext)
            .map_err(crypto_error)?;
        Ok((
            chunk.transfer_id,
            chunk.chunk_index,
            Zeroizing::new(plaintext),
        ))
    }

    pub fn decrypt_close(&mut self, payload: &[u8]) -> Result<SecureCloseData, String> {
        let close = decode_secure_close(payload).map_err(protocol_error)?;
        let State::Established {
            session_id, files, ..
        } = &mut self.state
        else {
            return Err("encrypted file received without an authenticated session".into());
        };
        if close.session_id != *session_id {
            return Err("file close does not belong to the active session".into());
        }
        let cipher = files
            .remove(&close.transfer_id)
            .ok_or_else(|| "close received before its authenticated manifest".to_string())?;
        let plaintext = Zeroizing::new(
            cipher
                .open(RECORD_CLOSE, 0, close.ciphertext)
                .map_err(crypto_error)?,
        );
        let (total_size, chunk_count, sha256) = decode_close(&plaintext).map_err(crypto_error)?;
        Ok(SecureCloseData {
            transfer_id: close.transfer_id,
            total_size,
            chunk_count,
            sha256,
        })
    }

    pub fn seal_receipt(&mut self, transfer_id: u64, sha256: &[u8; 32]) -> Result<Vec<u8>, String> {
        let receipt = Zeroizing::new(encode_receipt(transfer_id, sha256));
        self.seal_control(&receipt)
    }

    pub fn discard_file(&mut self, transfer_id: u64) {
        if let State::Established { files, .. } = &mut self.state {
            files.remove(&transfer_id);
        }
    }

    fn seal_control(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let State::Established {
            session_id,
            transport,
            ..
        } = &mut self.state
        else {
            return Err("establish an encrypted session first".into());
        };
        let body = transport.seal(plaintext).map_err(crypto_error)?;
        encode_session(SESSION_KIND_TRANSPORT, session_id, &body)
    }
}

pub fn secure_transfer_id(kind: u8, payload: &[u8]) -> Option<u64> {
    match kind {
        transfer_protocol::WS_BINARY_KIND_SECURE_OPEN => decode_secure_open(payload)
            .ok()
            .map(|frame| frame.transfer_id),
        transfer_protocol::WS_BINARY_KIND_SECURE_CHUNK => decode_secure_chunk(payload)
            .ok()
            .map(|frame| frame.transfer_id),
        transfer_protocol::WS_BINARY_KIND_SECURE_CLOSE => decode_secure_close(payload)
            .ok()
            .map(|frame| frame.transfer_id),
        _ => None,
    }
}

fn encode_session(
    kind: u8,
    session_id: &[u8; SESSION_ID_LEN],
    body: &[u8],
) -> Result<Vec<u8>, String> {
    let needed = SESSION_ENVELOPE_HEADER_LEN
        .checked_add(body.len())
        .ok_or_else(|| "secure-session envelope is too large".to_string())?;
    if needed > MAX_SESSION_ENVELOPE {
        return Err("secure-session envelope is too large".into());
    }
    let mut out = vec![0; needed];
    let len = encode_session_envelope(&mut out, kind, session_id, body).map_err(protocol_error)?;
    out.truncate(len);
    Ok(out)
}

fn crypto_error(error: transfer_crypto::CryptoError) -> String {
    error.to_string()
}

fn protocol_error(error: transfer_protocol::DecodeError) -> String {
    format!("invalid encrypted-transfer frame: {error:?}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use transfer_crypto::HostHandshake;

    #[test]
    fn unattended_session_establishes_transport_for_encrypted_path_control() {
        let code = "00112233445566778899aabbccddeeff";
        let session_id = [0x35; SESSION_ID_LEN];
        let mut browser = SecureSession::new();

        let request = browser.request_session().unwrap();
        let request = decode_session_envelope(&request).unwrap();
        assert_eq!(request.kind, SESSION_KIND_REQUEST);

        let ready = encode_session(SESSION_KIND_READY, &session_id, code.as_bytes()).unwrap();
        let first = browser.handle_session(&ready).unwrap().unwrap();
        assert_eq!(browser.view(), SecureSessionView::Handshaking);
        let first = decode_session_envelope(&first).unwrap();
        let psk = derive_session_psk(code, &session_id).unwrap();
        let host = HostHandshake::new(&psk).unwrap();
        let (mut host, response) = host.respond(first.body).unwrap();

        let response = encode_session(SESSION_KIND_HANDSHAKE, &session_id, &response).unwrap();
        let sealed_master = browser.handle_session(&response).unwrap().unwrap();
        let sealed_master = decode_session_envelope(&sealed_master).unwrap();
        assert_eq!(host.open(sealed_master.body).unwrap().len(), 32);

        let confirmation = host.seal(SESSION_CONFIRMATION).unwrap();
        let confirmation =
            encode_session(SESSION_KIND_TRANSPORT, &session_id, &confirmation).unwrap();
        assert!(browser.handle_session(&confirmation).unwrap().is_none());
        assert!(browser.view().established());

        let path = browser.seal_start("/private/example.bin").unwrap();
        let path = decode_session_envelope(&path).unwrap();
        let path = host.open(path.body).unwrap();
        assert_eq!(path[0], CONTROL_START_TRANSFER);
        assert_eq!(&path[1..], b"/private/example.bin");
    }

    #[test]
    fn unattended_session_rejects_malformed_bootstrap_secret() {
        let session_id = [0x35; SESSION_ID_LEN];
        let mut browser = SecureSession::new();
        browser.request_session().unwrap();

        let ready = encode_session(SESSION_KIND_READY, &session_id, b"not-a-secret").unwrap();
        assert!(browser.handle_session(&ready).is_err());
        assert_eq!(browser.view(), SecureSessionView::Failed);
    }
}
