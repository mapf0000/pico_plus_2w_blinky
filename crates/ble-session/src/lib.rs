//! Allocation-free Noise sessions for Pico control. No radio or platform RNG.
#![no_std]

use noise_protocol::{Cipher, DH, HandshakeState, U8Array};
use noise_rust_crypto::{ChaCha20Poly1305, Sha256, sensitive::Sensitive};
use zeroize::Zeroize;

pub mod provisioning;
pub const SUITE: &str = "Noise_NNpsk0_25519_ChaChaPoly_SHA256";
pub const HANDSHAKE_LEN: usize = 48;
pub const TAG_LEN: usize = 16;
pub const MAX_PLAINTEXT: usize = 4126; // kind byte + unchanged script envelope
pub const MAX_CIPHERTEXT: usize = MAX_PLAINTEXT + TAG_LEN;

/// Compiler-resistant clearing for caller-owned plaintext/provisioning buffers.
pub fn erase(bytes: &mut [u8]) {
    bytes.zeroize();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Length,
    Authentication,
    Sequence,
    State,
}

pub fn prologue(device_id: &[u8; 16]) -> [u8; 32] {
    let mut value = [0; 32];
    value[..16].copy_from_slice(b"pico-control-v3:");
    value[16..].copy_from_slice(device_id);
    value
}

// The wrapper always installs a fresh, caller-generated ephemeral key. This
// private DH implementation cannot be constructed by consumers of this crate.
enum X25519 {}
impl DH for X25519 {
    type Key = Sensitive<[u8; 32]>;
    type Pubkey = [u8; 32];
    type Output = Sensitive<[u8; 32]>;
    fn name() -> &'static str {
        "25519"
    }
    fn genkey() -> Self::Key {
        unreachable!("Handshake::new always installs a caller-generated ephemeral key")
    }
    fn pubkey(key: &Self::Key) -> Self::Pubkey {
        let secret = x25519_dalek::StaticSecret::from(**key);
        x25519_dalek::PublicKey::from(&secret).to_bytes()
    }
    fn dh(key: &Self::Key, public: &Self::Pubkey) -> Result<Self::Output, ()> {
        let secret = x25519_dalek::StaticSecret::from(**key);
        let shared = secret.diffie_hellman(&x25519_dalek::PublicKey::from(*public));
        if !shared.was_contributory() {
            return Err(());
        }
        Ok(Sensitive::from_slice(shared.as_bytes()))
    }
}

pub struct Handshake {
    state: HandshakeState<X25519, ChaCha20Poly1305, Sha256>,
    initiator: bool,
    failed: bool,
}
impl Handshake {
    /// `ephemeral` MUST come from a cryptographic RNG for every new handshake.
    /// Never persist/reuse it. Platform randomness remains outside this crate.
    pub fn new(initiator: bool, profile: &provisioning::Profile, mut ephemeral: [u8; 32]) -> Self {
        let mut state = HandshakeState::new(
            noise_protocol::patterns::noise_nn_psk0(),
            initiator,
            prologue(&profile.device_id),
            None,
            Some(Sensitive::from_slice(&ephemeral)),
            None,
            None,
        );
        ephemeral.zeroize();
        state.push_psk(profile.key());
        Self {
            state,
            initiator,
            failed: false,
        }
    }
    pub fn write(&mut self, out: &mut [u8; HANDSHAKE_LEN]) -> Result<(), Error> {
        if self.failed || self.state.completed() || !self.state.is_write_turn() {
            return Err(Error::State);
        }
        self.state.write_message(&[], out).map_err(|_| {
            self.failed = true;
            Error::Authentication
        })
    }
    pub fn read(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if self.failed || self.state.completed() || self.state.is_write_turn() {
            return Err(Error::State);
        }
        if bytes.len() != HANDSHAKE_LEN {
            self.failed = true;
            return Err(Error::Length);
        }
        self.state.read_message(bytes, &mut []).map_err(|_| {
            self.failed = true;
            Error::Authentication
        })
    }
    pub fn finish(self) -> Result<Session, Error> {
        if self.failed || !self.state.completed() {
            return Err(Error::State);
        }
        let (first, second) = self.state.get_ciphers();
        let (first, _) = first.extract();
        let (second, _) = second.extract();
        let (tx, rx) = if self.initiator {
            (first, second)
        } else {
            (second, first)
        };
        Ok(Session {
            tx,
            rx,
            tx_sequence: 0,
            rx_sequence: 0,
        })
    }
}

/// Independent directional sequence numbers allow a newer Cancel to abandon an
/// incomplete Run. A complete authenticated record is accepted only once.
pub struct Session {
    tx: Sensitive<[u8; 32]>,
    rx: Sensitive<[u8; 32]>,
    tx_sequence: u16,
    rx_sequence: u16,
}
impl Session {
    pub fn seal(
        &mut self,
        sequence: u16,
        plaintext: &[u8],
        out: &mut [u8],
    ) -> Result<usize, Error> {
        if sequence == 0 || sequence <= self.tx_sequence {
            return Err(Error::Sequence);
        }
        if plaintext.is_empty()
            || plaintext.len() > MAX_PLAINTEXT
            || out.len() < plaintext.len() + TAG_LEN
        {
            return Err(Error::Length);
        }
        let size = plaintext.len() + TAG_LEN;
        let ad = [3, 4, sequence as u8, (sequence >> 8) as u8];
        ChaCha20Poly1305::encrypt(&self.tx, sequence.into(), &ad, plaintext, &mut out[..size]);
        self.tx_sequence = sequence;
        Ok(size)
    }
    pub fn open(
        &mut self,
        sequence: u16,
        ciphertext: &[u8],
        out: &mut [u8],
    ) -> Result<usize, Error> {
        if sequence == 0 || sequence <= self.rx_sequence {
            return Err(Error::Sequence);
        }
        if ciphertext.len() <= TAG_LEN
            || ciphertext.len() > MAX_CIPHERTEXT
            || out.len() < ciphertext.len() - TAG_LEN
        {
            return Err(Error::Length);
        }
        let size = ciphertext.len() - TAG_LEN;
        let ad = [3, 4, sequence as u8, (sequence >> 8) as u8];
        if ChaCha20Poly1305::decrypt(&self.rx, sequence.into(), &ad, ciphertext, &mut out[..size])
            .is_err()
        {
            out[..size].zeroize();
            return Err(Error::Authentication);
        }
        self.rx_sequence = sequence;
        Ok(size)
    }
}

#[cfg(test)]
mod tests;
