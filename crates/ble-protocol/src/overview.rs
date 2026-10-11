use crate::Error;

const HEADER: usize = 40;
const STRING_LIMITS: [usize; 4] = [96, 32, 24, 96];
pub const MAX_OVERVIEW_LEN: usize = HEADER + 96 + 32 + 24 + 96;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BootstrapPhase {
    Idle,
    Armed,
    Downloading,
    Verified,
    Failed,
    Cancelled,
}

impl BootstrapPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Armed => "Armed",
            Self::Downloading => "Downloading",
            Self::Verified => "Verified; waiting for agent",
            Self::Failed => "Failed; retry installation",
            Self::Cancelled => "Cancelled",
        }
    }
}

/// Finite metadata, independent of the streaming relay. Borrowed strings keep
/// encoding/decoding allocation-free; empty agent fields mean unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overview<'a> {
    pub request: u16,
    pub bootstrap: BootstrapPhase,
    pub bootstrap_available: bool,
    pub script_version: u8,
    pub filesystem_version: u16,
    pub transfer_version: u16,
    pub max_bytecode: u16,
    pub max_usb_payload: u16,
    pub agent_age_ms: Option<u64>,
    pub max_delay_ms: u32,
    pub max_total_delay_ms: u32,
    pub firmware: &'a str,
    pub agent_version: &'a str,
    pub host_os: &'a str,
    pub hostname: &'a str,
}

impl Overview<'_> {
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Error> {
        let strings = [
            self.firmware,
            self.agent_version,
            self.host_os,
            self.hostname,
        ];
        for (index, value) in strings.iter().enumerate() {
            validate(value, STRING_LIMITS[index], index == 0)?;
        }
        if self.request == 0 || self.max_bytecode == 0 || self.max_usb_payload == 0 {
            return Err(Error::Invalid);
        }
        let length = HEADER + strings.iter().map(|s| s.len()).sum::<usize>();
        if out.len() < length {
            return Err(Error::Length);
        }
        out[..length].fill(0);
        out[..4].copy_from_slice(b"POV1");
        out[4] = 1;
        out[5] = self.bootstrap as u8;
        out[6] = u8::from(self.bootstrap_available) | (u8::from(self.agent_age_ms.is_some()) << 1);
        out[7] = self.script_version;
        out[8..10].copy_from_slice(&self.filesystem_version.to_le_bytes());
        out[10..12].copy_from_slice(&self.transfer_version.to_le_bytes());
        out[12..14].copy_from_slice(&self.max_bytecode.to_le_bytes());
        out[14..16].copy_from_slice(&self.max_usb_payload.to_le_bytes());
        out[16..24].copy_from_slice(&self.agent_age_ms.unwrap_or(0).to_le_bytes());
        out[24..28].copy_from_slice(&self.max_delay_ms.to_le_bytes());
        out[28..32].copy_from_slice(&self.max_total_delay_ms.to_le_bytes());
        out[36..38].copy_from_slice(&self.request.to_le_bytes());
        let mut at = HEADER;
        for (i, value) in strings.iter().enumerate() {
            out[32 + i] = value.len() as u8;
            out[at..at + value.len()].copy_from_slice(value.as_bytes());
            at += value.len();
        }
        Ok(length)
    }
}

impl<'a> Overview<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER || bytes.len() > MAX_OVERVIEW_LEN {
            return Err(Error::Length);
        }
        if bytes[4] != 1 {
            return Err(Error::Version);
        }
        if &bytes[..4] != b"POV1" || bytes[6] & !3 != 0 || bytes[38..40] != [0, 0] {
            return Err(Error::Invalid);
        }
        let bootstrap = match bytes[5] {
            0 => BootstrapPhase::Idle,
            1 => BootstrapPhase::Armed,
            2 => BootstrapPhase::Downloading,
            3 => BootstrapPhase::Verified,
            4 => BootstrapPhase::Failed,
            5 => BootstrapPhase::Cancelled,
            _ => return Err(Error::Invalid),
        };
        let mut at = HEADER;
        let mut strings = [""; 4];
        for (i, value) in strings.iter_mut().enumerate() {
            let length = usize::from(bytes[32 + i]);
            let data = bytes.get(at..at + length).ok_or(Error::Length)?;
            *value = core::str::from_utf8(data).map_err(|_| Error::Invalid)?;
            validate(value, STRING_LIMITS[i], i == 0)?;
            at += length;
        }
        if at != bytes.len() {
            return Err(Error::Length);
        }
        let u16_at = |at| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        let mut age = [0; 8];
        age.copy_from_slice(&bytes[16..24]);
        let age = u64::from_le_bytes(age);
        if bytes[6] & 2 == 0 && age != 0 {
            return Err(Error::Invalid);
        }
        let request = u16_at(36);
        let max_bytecode = u16_at(12);
        let max_usb_payload = u16_at(14);
        if request == 0 || max_bytecode == 0 || max_usb_payload == 0 {
            return Err(Error::Invalid);
        }
        let mut delay = [0; 4];
        delay.copy_from_slice(&bytes[24..28]);
        let mut total = [0; 4];
        total.copy_from_slice(&bytes[28..32]);
        Ok(Self {
            request,
            bootstrap,
            bootstrap_available: bytes[6] & 1 != 0,
            script_version: bytes[7],
            filesystem_version: u16_at(8),
            transfer_version: u16_at(10),
            max_bytecode,
            max_usb_payload,
            agent_age_ms: (bytes[6] & 2 != 0).then_some(age),
            max_delay_ms: u32::from_le_bytes(delay),
            max_total_delay_ms: u32::from_le_bytes(total),
            firmware: strings[0],
            agent_version: strings[1],
            host_os: strings[2],
            hostname: strings[3],
        })
    }
}

fn validate(value: &str, limit: usize, required: bool) -> Result<(), Error> {
    if value.len() > limit
        || (required && value.is_empty())
        || !value.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
    {
        Err(Error::Invalid)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn value() -> Overview<'static> {
        Overview {
            request: 42,
            bootstrap: BootstrapPhase::Verified,
            bootstrap_available: true,
            script_version: 1,
            filesystem_version: 1,
            transfer_version: 2,
            max_bytecode: 4096,
            max_usb_payload: 2048,
            agent_age_ms: Some(25001),
            max_delay_ms: 60000,
            max_total_delay_ms: 300000,
            firmware: "0.1.0+full-build",
            agent_version: "0.1.0",
            host_os: "macos",
            hostname: "test-host",
        }
    }
    #[test]
    fn round_trip_absent_stale_and_maximum_metadata() {
        let build = [b'a'; 96];
        let version = [b'b'; 32];
        let os = [b'c'; 24];
        let host = [b'd'; 96];
        for meta in [
            value(),
            Overview {
                agent_age_ms: None,
                agent_version: "",
                host_os: "",
                hostname: "",
                ..value()
            },
            Overview {
                firmware: core::str::from_utf8(&build).unwrap(),
                agent_version: core::str::from_utf8(&version).unwrap(),
                host_os: core::str::from_utf8(&os).unwrap(),
                hostname: core::str::from_utf8(&host).unwrap(),
                ..value()
            },
        ] {
            let mut bytes = [0; MAX_OVERVIEW_LEN];
            let length = meta.encode(&mut bytes).unwrap();
            assert_eq!(Overview::decode(&bytes[..length]), Ok(meta));
            let cipher = [0; crate::MAX_RESPONSE_LEN];
            let mut response = crate::Response::new();
            response
                .set(crate::KIND_RECORD, 1, &cipher[..length + 16])
                .unwrap();
            let mut rx = crate::Receiver::new();
            for _ in 0..(length + 16).div_ceil(crate::PAYLOAD_LEN) {
                let frame = response.next_fragment().unwrap();
                rx.push(crate::read_fragment(&frame).unwrap()).unwrap();
            }
            assert!(response.next_fragment().is_err());
        }
    }
    #[test]
    fn reject_malformed_oversized_and_trailing_data() {
        let mut bytes = [0; MAX_OVERVIEW_LEN];
        let length = value().encode(&mut bytes).unwrap();
        for len in 0..length {
            assert!(Overview::decode(&bytes[..len]).is_err());
        }
        for index in [0, 4, 5, 6, 32, 33, 34, 35, 38, 39, 40] {
            let mut bad = bytes;
            bad[index] = 255;
            assert!(Overview::decode(&bad[..length]).is_err(), "byte {index}");
        }
        assert!(Overview::decode(&bytes[..length + 1]).is_err());
        let mut invalid = value();
        invalid.request = 0;
        assert!(invalid.encode(&mut bytes).is_err());
        invalid = value();
        invalid.firmware = "unsafe\ntext";
        assert!(invalid.encode(&mut bytes).is_err());
        let mut absent = value();
        absent.agent_age_ms = None;
        let length = absent.encode(&mut bytes).unwrap();
        bytes[16] = 1;
        assert!(Overview::decode(&bytes[..length]).is_err());
    }
}
