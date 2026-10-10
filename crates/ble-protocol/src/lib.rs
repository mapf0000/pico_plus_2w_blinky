//! Small read-only GATT values fitting the default ATT MTU. No command channel.
#![no_std]

pub const VERSION: u8 = 1;
pub const SERVICE_UUID: u128 = 0x7069636f_0001_4c32_9b89_5d7a00000001;
pub const INFO_UUID: u128 = 0x7069636f_0001_4c32_9b89_5d7a00000002;
pub const STATUS_UUID: u128 = 0x7069636f_0001_4c32_9b89_5d7a00000003;
pub const INFO_LEN: usize = 20;
pub const STATUS_LEN: usize = 12;
const READ_ONLY: u8 = 1;

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
        bytes[5] = READ_ONLY;
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
        if &bytes[..4] != b"PBLE" || bytes[5] != READ_ONLY || bytes[6..8] != [0, 0] {
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
