#![no_std]

#[cfg(test)]
extern crate std;

use heapless::Vec;

pub const HEADER_LEN: usize = 1 + 4;
pub const MAX_PAYLOAD: usize = 2048;
pub const MAX_FRAME: usize = HEADER_LEN + MAX_PAYLOAD;

#[repr(u8)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Tag {
    Execute = 1,
    DebugMsg = 2,
    WsConnect = 3,
    WsData = 4,
    WsDisconnect = 5,
    WsDataRecv = 6,
    RequestAgentStatus = 7,
    AgentStatus = 8,
    ExecuteResult = 9,
    MicPcmData = 10,
}

impl Tag {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    pub const fn from_u8(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Tag::Execute),
            2 => Some(Tag::DebugMsg),
            3 => Some(Tag::WsConnect),
            4 => Some(Tag::WsData),
            5 => Some(Tag::WsDisconnect),
            6 => Some(Tag::WsDataRecv),
            7 => Some(Tag::RequestAgentStatus),
            8 => Some(Tag::AgentStatus),
            9 => Some(Tag::ExecuteResult),
            10 => Some(Tag::MicPcmData),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Frame<'a> {
    pub tag: u8,
    pub payload: &'a [u8],
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum EncodeError {
    PayloadTooLarge,
    BufferTooSmall,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub struct DecodeStats {
    pub frames: u64,
    pub invalid_len: u64,
    pub invalid_tag: u64,
    pub overflow: u64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum DecodeState {
    Tag,
    Len { tag: u8, buf: [u8; 4], used: usize },
    Payload { tag: u8, len: usize },
}

impl Default for DecodeState {
    fn default() -> Self {
        DecodeState::Tag
    }
}

pub struct TlvDecoder {
    state: DecodeState,
    payload: Vec<u8, MAX_PAYLOAD>,
    accept_unknown: bool,
    stats: DecodeStats,
}

impl TlvDecoder {
    pub fn new(accept_unknown: bool) -> Self {
        Self {
            state: DecodeState::Tag,
            payload: Vec::new(),
            accept_unknown,
            stats: DecodeStats::default(),
        }
    }

    pub fn stats(&self) -> DecodeStats {
        self.stats
    }

    pub fn reset(&mut self) {
        self.state = DecodeState::Tag;
        self.payload.clear();
        self.stats = DecodeStats::default();
    }

    /// Feed raw bytes into the decoder.
    ///
    /// The payload slice handed to `on_frame` is only valid for the duration of
    /// the callback and will be reused on the next frame.
    pub fn push_bytes<F: FnMut(Frame)>(&mut self, data: &[u8], mut on_frame: F) {
        for &b in data {
            match self.state {
                DecodeState::Tag => {
                    if !self.accept_unknown && Tag::from_u8(b).is_none() {
                        self.stats.invalid_tag = self.stats.invalid_tag.saturating_add(1);
                        continue;
                    }
                    self.state = DecodeState::Len {
                        tag: b,
                        buf: [0u8; 4],
                        used: 0,
                    };
                }
                DecodeState::Len {
                    tag,
                    mut buf,
                    mut used,
                } => {
                    buf[used] = b;
                    used += 1;
                    if used == 4 {
                        let len = u32::from_le_bytes(buf) as usize;
                        if len > MAX_PAYLOAD {
                            self.stats.invalid_len = self.stats.invalid_len.saturating_add(1);
                            self.state = DecodeState::Tag;
                            continue;
                        }
                        if len == 0 {
                            self.stats.frames = self.stats.frames.saturating_add(1);
                            on_frame(Frame { tag, payload: &[] });
                            self.state = DecodeState::Tag;
                        } else {
                            self.payload.clear();
                            self.state = DecodeState::Payload { tag, len };
                        }
                    } else {
                        self.state = DecodeState::Len { tag, buf, used };
                    }
                }
                DecodeState::Payload { tag, len } => {
                    if self.payload.push(b).is_err() {
                        self.stats.overflow = self.stats.overflow.saturating_add(1);
                        self.payload.clear();
                        self.state = DecodeState::Tag;
                        continue;
                    }
                    if self.payload.len() == len {
                        self.stats.frames = self.stats.frames.saturating_add(1);
                        let payload = self.payload.as_slice();
                        on_frame(Frame { tag, payload });
                        self.payload.clear();
                        self.state = DecodeState::Tag;
                    }
                }
            }
        }
    }
}

pub fn encode_frame(tag: u8, payload: &[u8], out: &mut [u8]) -> Result<usize, EncodeError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(EncodeError::PayloadTooLarge);
    }
    let needed = HEADER_LEN + payload.len();
    if out.len() < needed {
        return Err(EncodeError::BufferTooSmall);
    }
    out[0] = tag;
    out[1..5].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    out[5..needed].copy_from_slice(payload);
    Ok(needed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_frame_writes_header_and_payload() {
        let mut out = [0u8; 64];
        let payload = [1u8, 2, 3, 4];
        let len = encode_frame(Tag::Execute.as_u8(), &payload, &mut out).unwrap();
        assert_eq!(len, HEADER_LEN + payload.len());
        assert_eq!(out[0], Tag::Execute.as_u8());
        let len_bytes = (payload.len() as u32).to_le_bytes();
        assert_eq!(&out[1..5], &len_bytes);
        assert_eq!(&out[5..9], &payload);
    }

    #[test]
    fn decoder_handles_empty_payload() {
        let mut buf = [0u8; 16];
        let len = encode_frame(Tag::RequestAgentStatus.as_u8(), &[], &mut buf).unwrap();
        let mut decoder = TlvDecoder::new(true);
        let mut seen = None;
        decoder.push_bytes(&buf[..len], |frame| {
            seen = Some((frame.tag, frame.payload.len()));
        });
        assert_eq!(seen, Some((Tag::RequestAgentStatus.as_u8(), 0)));
    }

    #[test]
    fn decoder_resyncs_after_invalid_length() {
        let mut buf = [0u8; 32];
        let valid_len = encode_frame(Tag::DebugMsg.as_u8(), b"ok", &mut buf).unwrap();

        let mut stream = [0u8; 16 + 32];
        stream[0] = Tag::Execute.as_u8();
        stream[1..5].copy_from_slice(&u32::MAX.to_le_bytes());
        stream[5..5 + valid_len].copy_from_slice(&buf[..valid_len]);

        let mut decoder = TlvDecoder::new(true);
        let mut seen: Option<(u8, std::vec::Vec<u8>)> = None;
        decoder.push_bytes(&stream[..5 + valid_len], |frame| {
            seen = Some((frame.tag, frame.payload.to_vec()));
        });
        assert_eq!(seen, Some((Tag::DebugMsg.as_u8(), b"ok".to_vec())));
        assert!(decoder.stats().invalid_len > 0);
    }

    #[test]
    fn encode_frame_rejects_oversize_payload() {
        let mut out = [0u8; MAX_FRAME];
        let payload = [0u8; MAX_PAYLOAD + 1];
        let err = encode_frame(Tag::Execute.as_u8(), &payload, &mut out).unwrap_err();
        assert_eq!(err, EncodeError::PayloadTooLarge);
    }

    #[test]
    fn encode_frame_rejects_small_buffer() {
        let payload = [1u8; 8];
        let mut out = [0u8; HEADER_LEN + 2];
        let err = encode_frame(Tag::Execute.as_u8(), &payload, &mut out).unwrap_err();
        assert_eq!(err, EncodeError::BufferTooSmall);
    }

    #[test]
    fn decoder_skips_unknown_tags_when_disabled() {
        let mut buf = [0u8; 32];
        let len = encode_frame(Tag::DebugMsg.as_u8(), b"ok", &mut buf).unwrap();
        let mut stream = [0u8; 64];
        stream[0] = 0xFF; // unknown tag
        stream[1..5].copy_from_slice(&2u32.to_le_bytes());
        stream[5] = 0xAA;
        stream[6] = 0xBB;
        stream[7..7 + len].copy_from_slice(&buf[..len]);

        let mut decoder = TlvDecoder::new(false);
        let mut seen: Option<(u8, std::vec::Vec<u8>)> = None;
        decoder.push_bytes(&stream[..7 + len], |frame| {
            seen = Some((frame.tag, frame.payload.to_vec()));
        });
        assert_eq!(seen, Some((Tag::DebugMsg.as_u8(), b"ok".to_vec())));
    }

    #[test]
    fn decoder_handles_incremental_input() {
        let mut buf = [0u8; 32];
        let len = encode_frame(Tag::Execute.as_u8(), b"ls", &mut buf).unwrap();
        let mut decoder = TlvDecoder::new(true);
        let mut seen: Option<(u8, std::vec::Vec<u8>)> = None;
        decoder.push_bytes(&buf[..2], |_| {});
        decoder.push_bytes(&buf[2..4], |_| {});
        decoder.push_bytes(&buf[4..len], |frame| {
            seen = Some((frame.tag, frame.payload.to_vec()));
        });
        assert_eq!(seen, Some((Tag::Execute.as_u8(), b"ls".to_vec())));
    }
}
