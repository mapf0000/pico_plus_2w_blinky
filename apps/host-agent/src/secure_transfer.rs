use anyhow::{Context, Result, bail};
use bytes::Bytes;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use transfer_crypto::{
    HostHandshake, HostTransport, SessionMaster, derive_session_psk, fill_random,
    generate_bootstrap_secret,
};
use transfer_protocol::{
    CONTROL_SET_DEFAULT_PATH, CONTROL_START_DEFAULT_TRANSFER, CONTROL_START_TRANSFER,
    CONTROL_TRANSFER_RECEIPT, SESSION_ID_LEN, SESSION_KIND_HANDSHAKE, SESSION_KIND_READY,
    SESSION_KIND_REQUEST, SESSION_KIND_TRANSPORT, TAG_TRANSFER_SESSION_TO_BROWSER,
    decode_session_envelope, encode_session_envelope,
};
use zeroize::Zeroizing;

const READY_TEXT: &[u8] = b"pico-transfer-ready-v1";
const MAX_SESSION_FRAME: usize = transfer_protocol::MAX_SECURE_SESSION_FRAME;
const NEGOTIATION_LIFETIME: Duration = Duration::from_secs(5 * 60);
const SESSION_REQUEST_MIN_INTERVAL: Duration = Duration::from_secs(2);

pub enum SecureAction {
    Send(Bytes),
    Established,
    StartTransfer(PathBuf),
    StartDefaultTransfer,
    SetDefaultPath(PathBuf),
    Receipt { transfer_id: u64, sha256: [u8; 32] },
}

enum SessionState {
    Idle,
    AwaitHandshake {
        session_id: [u8; SESSION_ID_LEN],
        handshake: Box<HostHandshake>,
        _bootstrap_secret: Zeroizing<String>,
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
    state: SessionState,
    last_session_request: Option<Instant>,
}

impl SecureTransferState {
    pub fn new() -> Self {
        Self {
            state: SessionState::Idle,
            last_session_request: None,
        }
    }

    pub fn session_material(&self) -> Option<([u8; SESSION_ID_LEN], SessionMaster)> {
        match &self.state {
            SessionState::Established {
                session_id, master, ..
            } => Some((*session_id, Zeroizing::new(**master))),
            _ => None,
        }
    }

    pub fn handle(&mut self, payload: &[u8]) -> Result<Vec<SecureAction>> {
        let envelope = decode_session_envelope(payload)
            .map_err(|_| anyhow::anyhow!("invalid secure session envelope"))?;
        match envelope.kind {
            SESSION_KIND_REQUEST => self.start_session(envelope.session_id, envelope.body),
            SESSION_KIND_HANDSHAKE => self.handle_handshake(envelope.session_id, envelope.body),
            SESSION_KIND_TRANSPORT => self.handle_transport(envelope.session_id, envelope.body),
            _ => bail!("unsupported secure session message"),
        }
    }

    fn start_session(
        &mut self,
        requested_session_id: [u8; SESSION_ID_LEN],
        body: &[u8],
    ) -> Result<Vec<SecureAction>> {
        if !body.is_empty() || requested_session_id != [0; SESSION_ID_LEN] {
            bail!("invalid session request");
        }
        let now = Instant::now();
        if self
            .last_session_request
            .is_some_and(|previous| now.duration_since(previous) < SESSION_REQUEST_MIN_INTERVAL)
        {
            bail!("session requests are rate limited");
        }
        self.last_session_request = Some(now);
        let mut session_id = [0; SESSION_ID_LEN];
        fill_random(&mut session_id).context("generate session id")?;
        let secret = generate_bootstrap_secret().context("generate bootstrap secret")?;
        let psk = derive_session_psk(&secret, &session_id).context("derive session key")?;
        let handshake = HostHandshake::new(&psk).context("initialize session handshake")?;
        let ready = encode(SESSION_KIND_READY, &session_id, secret.as_bytes())?;

        self.state = SessionState::AwaitHandshake {
            session_id,
            handshake: Box::new(handshake),
            _bootstrap_secret: secret,
            expires_at: now + NEGOTIATION_LIFETIME,
        };
        Ok(vec![SecureAction::Send(ready)])
    }

    fn handle_handshake(
        &mut self,
        session_id: [u8; SESSION_ID_LEN],
        body: &[u8],
    ) -> Result<Vec<SecureAction>> {
        let state = core::mem::replace(&mut self.state, SessionState::Idle);
        let SessionState::AwaitHandshake {
            session_id: expected,
            handshake,
            expires_at,
            ..
        } = state
        else {
            bail!("session handshake is not expected");
        };
        if session_id != expected {
            bail!("session handshake mismatch");
        }
        if Instant::now() > expires_at {
            bail!("session negotiation expired");
        }
        let (transport, response) = (*handshake)
            .respond(body)
            .context("complete session handshake")?;
        self.state = SessionState::AwaitMaster {
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
        let state = core::mem::replace(&mut self.state, SessionState::Idle);
        match state {
            SessionState::AwaitMaster {
                session_id: expected,
                mut transport,
                expires_at,
            } => {
                if session_id != expected {
                    bail!("session handshake mismatch");
                }
                if Instant::now() > expires_at {
                    bail!("session negotiation expired");
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
                self.state = SessionState::Established {
                    session_id,
                    transport,
                    master,
                };
                Ok(vec![
                    SecureAction::Send(encode(SESSION_KIND_TRANSPORT, &session_id, &confirmation)?),
                    SecureAction::Established,
                ])
            }
            SessionState::Established {
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
                self.state = SessionState::Established {
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

#[cfg(test)]
mod tests {
    use super::*;
    use transfer_protocol::{SESSION_KIND_READY, SESSION_KIND_REQUEST};

    #[test]
    fn session_request_returns_unattended_bootstrap_secret() {
        let mut request = [0; transfer_protocol::SESSION_ENVELOPE_HEADER_LEN];
        let request_len = encode_session_envelope(
            &mut request,
            SESSION_KIND_REQUEST,
            &[0; SESSION_ID_LEN],
            &[],
        )
        .unwrap();
        let mut state = SecureTransferState::new();

        let actions = state.handle(&request[..request_len]).unwrap();
        let SecureAction::Send(ready) = &actions[0] else {
            panic!("session request must return a ready envelope");
        };
        let ready = decode_session_envelope(ready).unwrap();
        assert_eq!(ready.kind, SESSION_KIND_READY);
        assert_ne!(ready.session_id, [0; SESSION_ID_LEN]);
        assert_eq!(ready.body.len(), 32);
        assert!(ready.body.iter().all(u8::is_ascii_hexdigit));
    }
}
