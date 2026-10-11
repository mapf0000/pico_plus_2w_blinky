//! Acquired encrypted USB identity requests and finite correlated responses.
use crate::Error;

pub const VERSION: u8 = 1;
pub const MANUFACTURER_MAX: usize = 32;
pub const PRODUCT_MAX: usize = 48;
pub const PAYLOAD_MAX: usize = 2 + MANUFACTURER_MAX + PRODUCT_MAX;
pub const RESPONSE_MAX: usize = 8 + PAYLOAD_MAX;
pub const COMMAND_MAX: usize = 2 + PAYLOAD_MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity<'a> {
    pub manufacturer: &'a str,
    pub product: &'a str,
}
impl Identity<'_> {
    pub fn valid(&self) -> bool {
        self.manufacturer.len() <= MANUFACTURER_MAX
            && self.product.len() <= PRODUCT_MAX
            && [self.manufacturer, self.product]
                .iter()
                .all(|s| s.bytes().all(|b| matches!(b, 32..=126)))
    }
    /// Same length-prefixed payload as the existing USB flash schema.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Error> {
        if !self.valid() {
            return Err(Error::Invalid);
        }
        let length = 2 + self.manufacturer.len() + self.product.len();
        if out.len() < length {
            return Err(Error::Length);
        }
        out[0] = self.manufacturer.len() as u8;
        let end = 1 + self.manufacturer.len();
        out[1..end].copy_from_slice(self.manufacturer.as_bytes());
        out[end] = self.product.len() as u8;
        out[end + 1..length].copy_from_slice(self.product.as_bytes());
        Ok(length)
    }
}
impl<'a> Identity<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, Error> {
        let mlen = usize::from(*bytes.first().ok_or(Error::Length)?);
        let plen = usize::from(*bytes.get(1 + mlen).ok_or(Error::Length)?);
        if mlen > MANUFACTURER_MAX || plen > PRODUCT_MAX {
            return Err(Error::Invalid);
        }
        if bytes.len() != 2 + mlen + plen {
            return Err(Error::Length);
        }
        let value = Self {
            manufacturer: core::str::from_utf8(&bytes[1..1 + mlen]).map_err(|_| Error::Invalid)?,
            product: core::str::from_utf8(&bytes[2 + mlen..]).map_err(|_| Error::Invalid)?,
        };
        if !value.valid() {
            return Err(Error::Invalid);
        }
        Ok(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command<'a> {
    Read,
    Save(Identity<'a>),
}
impl Command<'_> {
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Error> {
        if out.len() < 2 {
            return Err(Error::Length);
        }
        out[0] = VERSION;
        match self {
            Self::Read => {
                out[1] = 0;
                Ok(2)
            }
            Self::Save(identity) => {
                out[1] = 1;
                Ok(2 + identity.encode(&mut out[2..])?)
            }
        }
    }
}
impl<'a> Command<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < 2 {
            return Err(Error::Length);
        }
        if bytes[0] != VERSION {
            return Err(Error::Version);
        }
        match bytes[1] {
            0 if bytes.len() == 2 => Ok(Self::Read),
            1 => Ok(Self::Save(Identity::decode(&bytes[2..])?)),
            _ => Err(Error::Invalid),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Success,
    UsbActive,
    Busy,
    Invalid,
    FlashFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Response<'a> {
    pub request: u16,
    pub status: Status,
    pub identity: Identity<'a>,
}
impl Response<'_> {
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Error> {
        if out.len() < 8 {
            return Err(Error::Length);
        }
        if self.request == 0 {
            return Err(Error::Invalid);
        }
        out[..4].copy_from_slice(b"PCF1");
        out[4] = VERSION;
        out[5] = self.status as u8;
        out[6..8].copy_from_slice(&self.request.to_le_bytes());
        Ok(8 + self.identity.encode(&mut out[8..])?)
    }
}
impl<'a> Response<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < 10 || bytes.len() > RESPONSE_MAX {
            return Err(Error::Length);
        }
        if bytes[4] != VERSION {
            return Err(Error::Version);
        }
        if &bytes[..4] != b"PCF1" {
            return Err(Error::Invalid);
        }
        let request = u16::from_le_bytes([bytes[6], bytes[7]]);
        if request == 0 {
            return Err(Error::Invalid);
        }
        let status = match bytes[5] {
            0 => Status::Success,
            1 => Status::UsbActive,
            2 => Status::Busy,
            3 => Status::Invalid,
            4 => Status::FlashFailure,
            _ => return Err(Error::Invalid),
        };
        Ok(Self {
            request,
            status,
            identity: Identity::decode(&bytes[8..])?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_round_trips_empty_and_maximum_identity_and_all_statuses() {
        let max_m = [b'M'; MANUFACTURER_MAX];
        let max_p = [b'P'; PRODUCT_MAX];
        for identity in [
            Identity {
                manufacturer: "",
                product: "",
            },
            Identity {
                manufacturer: "Pico 2W",
                product: "Logger + Keyboard",
            },
            Identity {
                manufacturer: core::str::from_utf8(&max_m).unwrap(),
                product: core::str::from_utf8(&max_p).unwrap(),
            },
        ] {
            let mut bytes = [0; RESPONSE_MAX];
            for command in [Command::Read, Command::Save(identity)] {
                let len = command.encode(&mut bytes).unwrap();
                assert_eq!(Command::decode(&bytes[..len]), Ok(command));
                assert!(Command::decode(&bytes[..len + 1]).is_err());
            }
            for status in [
                Status::Success,
                Status::UsbActive,
                Status::Busy,
                Status::Invalid,
                Status::FlashFailure,
            ] {
                let response = Response {
                    request: u16::MAX,
                    status,
                    identity,
                };
                let len = response.encode(&mut bytes).unwrap();
                assert_eq!(Response::decode(&bytes[..len]), Ok(response));
                for length in 0..len {
                    assert!(Response::decode(&bytes[..length]).is_err());
                }
            }
        }
    }
    #[test]
    fn malformed_fields_and_oversize_are_rejected() {
        let mut bytes = [0; RESPONSE_MAX + 1];
        let response = Response {
            request: 1,
            status: Status::Success,
            identity: Identity {
                manufacturer: "Pico",
                product: "Keyboard",
            },
        };
        let len = response.encode(&mut bytes).unwrap();
        for index in [0, 4, 5, 8, 9, 13] {
            let mut bad = bytes;
            bad[index] = 255;
            assert!(Response::decode(&bad[..len]).is_err());
        }
        assert!(Response::decode(&bytes[..len + 1]).is_err());
        assert!(
            Response {
                request: 0,
                ..response
            }
            .encode(&mut bytes)
            .is_err()
        );
        for invalid in ["line\nbreak", "non-ascii é", "\0", "\u{7f}"] {
            assert!(
                !Identity {
                    manufacturer: invalid,
                    product: ""
                }
                .valid()
            );
        }
        let too_long = [b'M'; MANUFACTURER_MAX + 1];
        assert!(
            !Identity {
                manufacturer: core::str::from_utf8(&too_long).unwrap(),
                product: ""
            }
            .valid()
        );
        assert!(Command::decode(&[2, 0]).is_err());
        assert!(Command::decode(&[1, 2]).is_err());
    }
}
