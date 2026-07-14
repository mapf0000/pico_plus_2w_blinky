use anyhow::{Context, Result, bail};
use bytes::Bytes;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use transfer_crypto::{
    HostHandshake, HostTransport, SessionMaster, derive_pairing_psk, fill_random,
    generate_pairing_code,
};
use transfer_protocol::{
    CONTROL_SET_DEFAULT_PATH, CONTROL_START_DEFAULT_TRANSFER, CONTROL_START_TRANSFER,
    CONTROL_TRANSFER_RECEIPT, PAIRING_ID_LEN, SESSION_ID_LEN, SESSION_KIND_HANDSHAKE,
    SESSION_KIND_PAIR_READY, SESSION_KIND_PAIR_REQUEST, SESSION_KIND_TRANSPORT,
    TAG_TRANSFER_SESSION_TO_BROWSER, decode_session_envelope, encode_session_envelope,
};
use zeroize::Zeroizing;

const READY_TEXT: &[u8] = b"pico-transfer-ready-v1";
const MAX_SESSION_FRAME: usize = transfer_protocol::MAX_SECURE_SESSION_FRAME;
const PAIRING_LIFETIME: Duration = Duration::from_secs(5 * 60);
const PAIRING_REQUEST_MIN_INTERVAL: Duration = Duration::from_secs(2);

pub enum SecureAction {
    Send(Bytes),
    Established,
    StartTransfer(PathBuf),
    StartDefaultTransfer,
    SetDefaultPath(PathBuf),
    Receipt { transfer_id: u64, sha256: [u8; 32] },
}

enum PairingState {
    Idle,
    AwaitHandshake {
        session_id: [u8; SESSION_ID_LEN],
        handshake: Box<HostHandshake>,
        _code: Zeroizing<String>,
        expires_at: Instant,
    },
    AwaitMaster {
        session_id: [u8; SESSION_ID_LEN],
        transport: HostTransport,
        expires_at: Instant,
    },
    Established {
        session_id: [u8; SESSION_ID_LEN],
        transport: HostTransport,
        master: SessionMaster,
    },
}

pub struct SecureTransferState {
    state: PairingState,
    last_pairing_request: Option<Instant>,
}

impl SecureTransferState {
    pub fn new() -> Self {
        Self {
            state: PairingState::Idle,
            last_pairing_request: None,
        }
    }

    pub fn session_material(&self) -> Option<([u8; SESSION_ID_LEN], SessionMaster)> {
        match &self.state {
            PairingState::Established {
                session_id, master, ..
            } => Some((*session_id, Zeroizing::new(**master))),
            _ => None,
        }
    }

    pub fn handle(&mut self, payload: &[u8]) -> Result<Vec<SecureAction>> {
        let envelope = decode_session_envelope(payload)
            .map_err(|_| anyhow::anyhow!("invalid secure session envelope"))?;
        match envelope.kind {
            SESSION_KIND_PAIR_REQUEST => self.start_pairing(envelope.session_id, envelope.body),
            SESSION_KIND_HANDSHAKE => self.handle_handshake(envelope.session_id, envelope.body),
            SESSION_KIND_TRANSPORT => self.handle_transport(envelope.session_id, envelope.body),
            _ => bail!("unsupported secure session message"),
        }
    }

    fn start_pairing(
        &mut self,
        requested_session_id: [u8; SESSION_ID_LEN],
        body: &[u8],
    ) -> Result<Vec<SecureAction>> {
        if !body.is_empty() || requested_session_id != [0; SESSION_ID_LEN] {
            bail!("invalid pair request");
        }
        let now = Instant::now();
        if self
            .last_pairing_request
            .is_some_and(|previous| now.duration_since(previous) < PAIRING_REQUEST_MIN_INTERVAL)
        {
            bail!("pairing requests are rate limited");
        }
        self.last_pairing_request = Some(now);
        let mut session_id = [0; SESSION_ID_LEN];
        fill_random(&mut session_id).context("generate pairing id")?;
        let code = generate_pairing_code().context("generate pairing code")?;
        let psk = derive_pairing_psk(&code, &session_id).context("derive pairing key")?;
        let handshake = HostHandshake::new(&psk).context("initialize pairing handshake")?;

        // Deliberately bypass tracing so the single-use code is not copied to log sinks.
        eprintln!(
            "\nPico secure file-transfer pairing code: {}\n",
            code.as_str()
        );

        self.state = PairingState::AwaitHandshake {
            session_id,
            handshake: Box::new(handshake),
            _code: code,
            expires_at: now + PAIRING_LIFETIME,
        };
        Ok(vec![SecureAction::Send(encode(
            SESSION_KIND_PAIR_READY,
            &session_id,
            &[],
        )?)])
    }

    fn handle_handshake(
        &mut self,
        session_id: [u8; SESSION_ID_LEN],
        body: &[u8],
    ) -> Result<Vec<SecureAction>> {
        let state = core::mem::replace(&mut self.state, PairingState::Idle);
        let PairingState::AwaitHandshake {
            session_id: expected,
            handshake,
            expires_at,
            ..
        } = state
        else {
            bail!("pairing handshake is not expected");
        };
        if session_id != expected {
            bail!("pairing session mismatch");
        }
        if Instant::now() > expires_at {
            bail!("pairing code expired");
        }
        let (transport, response) = (*handshake)
            .respond(body)
            .context("complete pairing handshake")?;
        self.state = PairingState::AwaitMaster {
            session_id,
            transport,
            expires_at,
        };
        Ok(vec![SecureAction::Send(encode(
            SESSION_KIND_HANDSHAKE,
            &session_id,
            &response,
        )?)])
    }

    fn handle_transport(
        &mut self,
        session_id: [u8; SESSION_ID_LEN],
        body: &[u8],
    ) -> Result<Vec<SecureAction>> {
        let state = core::mem::replace(&mut self.state, PairingState::Idle);
        match state {
            PairingState::AwaitMaster {
                session_id: expected,
                mut transport,
                expires_at,
            } => {
                if session_id != expected {
                    bail!("pairing session mismatch");
                }
                if Instant::now() > expires_at {
                    bail!("pairing code expired");
                }
                let plaintext =
                    Zeroizing::new(transport.open(body).context("open session master")?);
                let master_bytes: [u8; 32] = plaintext
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("invalid session master"))?;
                let master = Zeroizing::new(master_bytes);
                let confirmation = transport
                    .seal(READY_TEXT)
                    .context("seal session confirmation")?;
                self.state = PairingState::Established {
                    session_id,
                    transport,
                    master,
                };
                Ok(vec![
                    SecureAction::Send(encode(SESSION_KIND_TRANSPORT, &session_id, &confirmation)?),
                    SecureAction::Established,
                ])
            }
            PairingState::Established {
                session_id: expected,
                mut transport,
                master,
            } => {
                if session_id != expected {
                    bail!("secure session mismatch");
                }
                let plaintext = Zeroizing::new(
                    transport
                        .open(body)
                        .context("open secure transfer control")?,
                );
                let action = decode_control(&plaintext)?;
                self.state = PairingState::Established {
                    session_id,
                    transport,
                    master,
                };
                Ok(vec![action])
            }
            other => {
                self.state = other;
                bail!("secure transport message is not expected")
            }
        }
    }
}

fn decode_control(plaintext: &[u8]) -> Result<SecureAction> {
    let Some((&kind, body)) = plaintext.split_first() else {
        bail!("empty secure transfer control");
    };
    match kind {
        CONTROL_START_TRANSFER | CONTROL_SET_DEFAULT_PATH => {
            if body.is_empty() || body.len() > transfer_protocol::MAX_TRANSFER_PATH_LEN {
                bail!("invalid secure transfer path");
            }
            let path = std::str::from_utf8(body).context("secure transfer path is not UTF-8")?;
            let path = PathBuf::from(path);
            if kind == CONTROL_START_TRANSFER {
                Ok(SecureAction::StartTransfer(path))
            } else {
                Ok(SecureAction::SetDefaultPath(path))
            }
        }
        CONTROL_TRANSFER_RECEIPT => {
            if body.len() != 8 + 32 {
                bail!("invalid secure transfer receipt");
            }
            let transfer_id = u64::from_le_bytes(body[0..8].try_into()?);
            let mut sha256 = [0; 32];
            sha256.copy_from_slice(&body[8..]);
            Ok(SecureAction::Receipt {
                transfer_id,
                sha256,
            })
        }
        CONTROL_START_DEFAULT_TRANSFER if body.is_empty() => Ok(SecureAction::StartDefaultTransfer),
        _ => bail!("unknown secure transfer control"),
    }
}

fn encode(kind: u8, session_id: &[u8; SESSION_ID_LEN], body: &[u8]) -> Result<Bytes> {
    let mut frame = [0; MAX_SESSION_FRAME];
    let len = encode_session_envelope(&mut frame, kind, session_id, body)
        .map_err(|_| anyhow::anyhow!("secure session response is too large"))?;
    Ok(Bytes::copy_from_slice(&frame[..len]))
}

pub const TO_BROWSER_TAG: u8 = TAG_TRANSFER_SESSION_TO_BROWSER;

const _: () = assert!(PAIRING_ID_LEN == SESSION_ID_LEN);
