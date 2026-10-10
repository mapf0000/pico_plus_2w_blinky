//! Bounded Bluetooth control protocol; every ATT write fits the default MTU.
#![no_std]

pub const VERSION: u8 = 2;
pub const SERVICE_UUID: u128 = 0x7069636f_0001_4c32_9b89_5d7a00000001;
pub const INFO_UUID: u128 = 0x7069636f_0001_4c32_9b89_5d7a00000002;
pub const STATUS_UUID: u128 = 0x7069636f_0001_4c32_9b89_5d7a00000003;
pub const INFO_LEN: usize = 20;
pub const STATUS_LEN: usize = 12;
const CONTROL: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Length,
    Version,
    Invalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    /// ASCII build label, truncated to 12 bytes and zero padded. Not device identity.
    pub build: [u8; 12],
}

impl Info {
    pub fn new(build: &str) -> Self {
        let mut value = Self { build: [0; 12] };
        for (out, input) in value.build.iter_mut().zip(build.bytes()) {
            *out = if input.is_ascii_graphic() {
                input
            } else {
                b'_'
            };
        }
        // Build generators normally supply a nonempty label.
        if value.build[0] == 0 {
            value.build[..7].copy_from_slice(b"unknown");
        }
        value
    }

    pub fn encode(self) -> [u8; INFO_LEN] {
        let mut bytes = [0; INFO_LEN];
        bytes[..4].copy_from_slice(b"PBLE");
        bytes[4] = VERSION;
        bytes[5] = CONTROL;
        bytes[8..].copy_from_slice(&self.build);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != INFO_LEN {
            return Err(Error::Length);
        }
        if bytes[4] != VERSION {
            return Err(Error::Version);
        }
        if &bytes[..4] != b"PBLE" || bytes[5] != CONTROL || bytes[6..8] != [0, 0] {
            return Err(Error::Invalid);
        }
        let mut build = [0; 12];
        build.copy_from_slice(&bytes[8..]);
        let label = Self { build };
        let end = label.build.iter().position(|b| *b == 0).unwrap_or(12);
        if end == 0
            || !label.build[..end].iter().all(u8::is_ascii_graphic)
            || label.build[end..].iter().any(|b| *b != 0)
        {
            return Err(Error::Invalid);
        }
        Ok(label)
    }

    pub fn build_label(&self) -> &str {
        let end = self.build.iter().position(|b| *b == 0).unwrap_or(12);
        core::str::from_utf8(&self.build[..end]).unwrap_or("invalid")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub usb_enabled: bool,
    pub usb_ready: bool,
    pub host_agent_present: bool,
    pub uptime_secs: u64,
}

impl Status {
    pub fn encode(self) -> [u8; STATUS_LEN] {
        let mut bytes = [0; STATUS_LEN];
        bytes[0] = VERSION;
        bytes[1] = u8::from(self.usb_enabled)
            | (u8::from(self.usb_ready) << 1)
            | (u8::from(self.host_agent_present) << 2);
        bytes[4..].copy_from_slice(&self.uptime_secs.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != STATUS_LEN {
            return Err(Error::Length);
        }
        if bytes[0] != VERSION {
            return Err(Error::Version);
        }
        if bytes[1] & !7 != 0 || bytes[2..4] != [0, 0] {
            return Err(Error::Invalid);
        }
        let mut uptime = [0; 8];
        uptime.copy_from_slice(&bytes[4..]);
        Ok(Self {
            usb_enabled: bytes[1] & 1 != 0,
            usb_ready: bytes[1] & 2 != 0,
            host_agent_present: bytes[1] & 4 != 0,
            uptime_secs: u64::from_le_bytes(uptime),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_fit_default_att_and_round_trip() {
        const {
            assert!(INFO_LEN <= 22 && STATUS_LEN <= 22);
        }
        let info = Info::new("0123456789abcdef");
        assert_eq!(info.build_label(), "0123456789ab");
        assert_eq!(Info::decode(&info.encode()), Ok(info));
        for flags in 0..8 {
            let status = Status {
                usb_enabled: flags & 1 != 0,
                usb_ready: flags & 2 != 0,
                host_agent_present: flags & 4 != 0,
                uptime_secs: u64::MAX,
            };
            assert_eq!(Status::decode(&status.encode()), Ok(status));
        }
    }

    #[test]
    fn malformed_values_are_rejected() {
        let valid = Info::new("test").encode();
        for len in 0..INFO_LEN {
            assert_eq!(Info::decode(&valid[..len]), Err(Error::Length));
        }
        for index in [0, 4, 5, 6, 7, 8, 13] {
            let mut value = valid;
            value[index] = 0xff;
            assert!(Info::decode(&value).is_err(), "byte {index}");
        }
        let valid = Status {
            usb_enabled: true,
            usb_ready: true,
            host_agent_present: false,
            uptime_secs: 42,
        }
        .encode();
        for len in 0..STATUS_LEN {
            assert_eq!(Status::decode(&valid[..len]), Err(Error::Length));
        }
        for index in 0..4 {
            let mut value = valid;
            value[index] = 0xff;
            assert!(Status::decode(&value).is_err());
        }
        let mut trailing = [0; INFO_LEN + 1];
        trailing[..INFO_LEN].copy_from_slice(&Info::new("test").encode());
        assert_eq!(Info::decode(&trailing), Err(Error::Length));
    }
}

pub const PAIR_UUID: u128 = 0x7069636f_0001_4c32_9b89_5d7a00000004;
pub const COMMAND_UUID: u128 = 0x7069636f_0001_4c32_9b89_5d7a00000005;
pub const RESULT_UUID: u128 = 0x7069636f_0001_4c32_9b89_5d7a00000006;
pub const FRAME_LEN: usize = 20;
pub const HEADER_LEN: usize = 8;
pub const PAYLOAD_LEN: usize = FRAME_LEN - HEADER_LEN;
pub const MAX_MESSAGE_LEN: usize = 4125;
pub const KIND_CONTROL: u8 = 1;
pub const KIND_SCRIPT: u8 = 2;
pub const ACQUIRE: u8 = 0;
pub const RELEASE: u8 = 1;
pub const USB_ON: u8 = 2;
pub const USB_OFF: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Code {
    Idle = 0,
    Acquired = 1,
    Released = 2,
    Accepted = 3,
    Completed = 4,
    Cancelled = 5,
    Rejected = 6,
    UsbUnavailable = 7,
    Busy = 8,
    UsbChanged = 9,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResultValue {
    pub token: u16,
    pub code: Code,
}
impl ResultValue {
    pub fn encode(self) -> [u8; FRAME_LEN] {
        let mut out = [0; FRAME_LEN];
        out[0] = VERSION;
        out[1] = self.code as u8;
        out[2..4].copy_from_slice(&self.token.to_le_bytes());
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != FRAME_LEN {
            return Err(Error::Length);
        }
        if bytes[0] != VERSION {
            return Err(Error::Version);
        }
        if bytes[4..].iter().any(|b| *b != 0) {
            return Err(Error::Invalid);
        }
        let code = match bytes[1] {
            0 => Code::Idle,
            1 => Code::Acquired,
            2 => Code::Released,
            3 => Code::Accepted,
            4 => Code::Completed,
            5 => Code::Cancelled,
            6 => Code::Rejected,
            7 => Code::UsbUnavailable,
            8 => Code::Busy,
            9 => Code::UsbChanged,
            _ => return Err(Error::Invalid),
        };
        let token = u16::from_le_bytes([bytes[2], bytes[3]]);
        if (token == 0) != (code == Code::Idle) {
            return Err(Error::Invalid);
        }
        Ok(Self { token, code })
    }
}

pub fn fragment(
    kind: u8,
    token: u16,
    message: &[u8],
    offset: usize,
) -> Result<([u8; FRAME_LEN], usize), Error> {
    if token == 0
        || !matches!(kind, KIND_CONTROL | KIND_SCRIPT)
        || message.is_empty()
        || message.len() > MAX_MESSAGE_LEN
        || offset >= message.len()
    {
        return Err(Error::Invalid);
    }
    let length = (message.len() - offset).min(PAYLOAD_LEN);
    let mut out = [0; FRAME_LEN];
    out[0] = VERSION;
    out[1] = kind;
    out[2..4].copy_from_slice(&token.to_le_bytes());
    out[4..6].copy_from_slice(&(offset as u16).to_le_bytes());
    out[6..8].copy_from_slice(&(message.len() as u16).to_le_bytes());
    out[8..8 + length].copy_from_slice(&message[offset..offset + length]);
    Ok((out, HEADER_LEN + length))
}

/// One connection, monotonic nonzero tokens, exact contiguous fragments.
/// A newer token abandons an incomplete upload. Tokens are never reused.
pub struct Message<'a> {
    pub token: u16,
    pub kind: u8,
    pub payload: &'a [u8],
}

pub struct Receiver {
    buffer: [u8; MAX_MESSAGE_LEN],
    token: u16,
    kind: u8,
    total: usize,
    received: usize,
}
impl Default for Receiver {
    fn default() -> Self {
        Self::new()
    }
}
impl Receiver {
    pub const fn new() -> Self {
        Self {
            buffer: [0; MAX_MESSAGE_LEN],
            token: 0,
            kind: 0,
            total: 0,
            received: 0,
        }
    }
    pub fn abandon(&mut self) {
        self.total = 0;
        self.received = 0;
    }
    pub fn push<'a>(&'a mut self, bytes: &[u8]) -> Result<Option<Message<'a>>, Error> {
        if bytes.len() <= HEADER_LEN || bytes.len() > FRAME_LEN {
            return Err(Error::Length);
        }
        if bytes[0] != VERSION {
            return Err(Error::Version);
        }
        let kind = bytes[1];
        let token = u16::from_le_bytes([bytes[2], bytes[3]]);
        let offset = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        let total = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        let payload = &bytes[HEADER_LEN..];
        if token == 0
            || !matches!(kind, KIND_CONTROL | KIND_SCRIPT)
            || total == 0
            || total > MAX_MESSAGE_LEN
            || offset + payload.len() > total
        {
            return Err(Error::Invalid);
        }
        if token > self.token {
            if offset != 0 {
                return Err(Error::Invalid);
            }
            self.token = token;
            self.kind = kind;
            self.total = total;
            self.received = 0;
        }
        if token != self.token
            || kind != self.kind
            || total != self.total
            || offset != self.received
        {
            return Err(Error::Invalid);
        }
        self.buffer[offset..offset + payload.len()].copy_from_slice(payload);
        self.received += payload.len();
        if self.received == self.total {
            self.total = 0;
            Ok(Some(Message {
                token,
                kind,
                payload: &self.buffer[..self.received],
            }))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod control_tests {
    use super::*;
    #[test]
    fn maximum_upload_and_default_mtu() {
        let input = [0x5a; MAX_MESSAGE_LEN];
        let mut rx = Receiver::new();
        for offset in (0..input.len()).step_by(PAYLOAD_LEN) {
            let (frame, len) = fragment(KIND_SCRIPT, 1, &input, offset).unwrap();
            assert!(len <= 20);
            let done = rx.push(&frame[..len]).unwrap();
            if offset + PAYLOAD_LEN >= input.len() {
                assert_eq!(done.unwrap().payload, input);
            } else {
                assert!(done.is_none());
            }
        }
    }
    #[test]
    fn reject_gaps_replays_and_resume_after_abandon() {
        let mut rx = Receiver::new();
        let input = [1; 30];
        let (first, len) = fragment(KIND_SCRIPT, 1, &input, 0).unwrap();
        assert!(rx.push(&first[..len]).unwrap().is_none());
        assert!(rx.push(&first[..len]).is_err());
        let (gap, len) = fragment(KIND_SCRIPT, 1, &input, 24).unwrap();
        assert!(rx.push(&gap[..len]).is_err());
        rx.abandon();
        let (resume, len) = fragment(KIND_SCRIPT, 1, &input, 12).unwrap();
        assert!(rx.push(&resume[..len]).is_err());
        let (cancel, len) = fragment(KIND_CONTROL, 2, &[RELEASE], 0).unwrap();
        assert_eq!(
            rx.push(&cancel[..len]).unwrap().unwrap().payload,
            &[RELEASE]
        );
        assert!(rx.push(&cancel[..len]).is_err());
    }
    #[test]
    fn invalid_headers_and_results() {
        let (frame, len) = fragment(KIND_CONTROL, 1, &[ACQUIRE], 0).unwrap();
        for i in [0, 1, 4, 5, 7] {
            let mut bad = frame;
            bad[i] = 255;
            assert!(Receiver::new().push(&bad[..len]).is_err());
        }
        for code in [
            Code::Acquired,
            Code::Accepted,
            Code::Completed,
            Code::Cancelled,
            Code::Busy,
        ] {
            let value = ResultValue { token: 42, code };
            assert_eq!(ResultValue::decode(&value.encode()), Ok(value));
        }
        let mut bytes = ResultValue {
            token: 42,
            code: Code::Completed,
        }
        .encode();
        bytes[19] = 1;
        assert!(ResultValue::decode(&bytes).is_err());
        assert!(fragment(KIND_SCRIPT, 0, &[1], 0).is_err());
    }
}
