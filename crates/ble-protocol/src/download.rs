//! Versioned file-transfer relay commands; opaque existing transfer-v2 records.
use crate::Error;
pub const VERSION: u8 = 1;
pub const RECORD_MAX: usize = 1 + transfer_protocol::TLV_MAX_PAYLOAD;
pub const COMMAND_MAX: usize = 2 + transfer_protocol::MAX_SECURE_SESSION_FRAME;
pub const SESSION: u8 = transfer_protocol::TAG_TRANSFER_SESSION_TO_BROWSER;
pub const OPEN: u8 = 20;
pub const CHUNK: u8 = 21;
pub const CLOSE: u8 = 23;
pub const ABORT: u8 = 25;
/// BLE-only cancellation barrier, never a USB tag.
pub const CANCELLED: u8 = 0;
pub enum Command<'a> {
    Session(&'a [u8]),
    Cancel,
}
impl<'a> Command<'a> {
    pub fn decode(b: &'a [u8]) -> Result<Self, Error> {
        match b {
            [VERSION, 0, body @ ..]
                if body.len() <= transfer_protocol::MAX_SECURE_SESSION_FRAME =>
            {
                transfer_protocol::decode_session_envelope(body).map_err(|_| Error::Invalid)?;
                Ok(Self::Session(body))
            }
            [VERSION, 1] => Ok(Self::Cancel),
            _ => Err(Error::Invalid),
        }
    }
}
pub fn record(b: &[u8]) -> Result<(u8, &[u8]), Error> {
    if b.len() > RECORD_MAX {
        return Err(Error::Length);
    }
    let (&tag, body) = b.split_first().ok_or(Error::Length)?;
    match tag {
        CANCELLED if body.is_empty() => {}
        SESSION => {
            transfer_protocol::decode_session_envelope(body).map_err(|_| Error::Invalid)?;
        }
        OPEN => {
            transfer_protocol::decode_secure_open(body).map_err(|_| Error::Invalid)?;
        }
        CHUNK => {
            transfer_protocol::decode_secure_chunk(body).map_err(|_| Error::Invalid)?;
        }
        CLOSE => {
            transfer_protocol::decode_secure_close(body).map_err(|_| Error::Invalid)?;
        }
        ABORT
            if body.len() >= 11
                && body.len() == 11 + usize::from(u16::from_le_bytes([body[9], body[10]])) =>
        {
            core::str::from_utf8(&body[11..]).map_err(|_| Error::Invalid)?;
        }
        _ => return Err(Error::Invalid),
    }
    Ok((tag, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maximum_chunk_fits_and_unknown_or_trailing_controls_fail() {
        let mut body = [0; transfer_protocol::TLV_MAX_PAYLOAD];
        transfer_protocol::encode_secure_chunk(
            &mut body,
            &[1; 16],
            2,
            0,
            &[3; transfer_protocol::MAX_PLAINTEXT_CHUNK + 16],
        )
        .unwrap();
        let mut record = [0; RECORD_MAX];
        record[0] = CHUNK;
        record[1..].copy_from_slice(&body);
        let mut out = [0; super::super::stream::PLAINTEXT_MAX];
        let n = super::super::stream::encode(super::super::stream::DOWNLOAD, &record, &mut out)
            .unwrap();
        assert_eq!(
            super::super::stream::decode(&out[..n]).unwrap().1.len(),
            RECORD_MAX
        );
        for command in [
            &[][..],
            &[1, 2][..],
            &[1, 1, 0][..],
            &[2, 1][..],
            &[1, 0][..],
        ] {
            assert!(Command::decode(command).is_err());
        }
        assert!(Command::decode(&[1, 1]).is_ok());
        assert!(self::record(&[CANCELLED]).is_ok());
        assert!(self::record(&[CANCELLED, 0]).is_err());
    }
}
