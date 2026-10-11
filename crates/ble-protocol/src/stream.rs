//! Credit-controlled event lane. All values fit ATT MTU 23.
use crate::{Error, FRAME_LEN, KIND_STREAM, PAYLOAD_LEN, fragment};
pub const VERSION: u8 = 1;
pub const WINDOW: u8 = 4;
pub const HEADER: usize = 6;
pub const PLAINTEXT_MAX: usize = HEADER + super::filesystem::PAGE_MAX;
pub const CIPHERTEXT_MAX: usize = PLAINTEXT_MAX + 16;
pub const FILESYSTEM: u8 = 1;
pub const BUTTON: u8 = 2;
pub const PROBE: u8 = 3;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Start,
    Ack { sequence: u16, offset: u16 },
    Probe,
}
impl Flow {
    pub fn encode(self) -> ([u8; 7], usize) {
        let mut b = [0; 7];
        b[0] = VERSION;
        match self {
            Self::Start => (b, 2),
            Self::Probe => {
                b[1] = 2;
                (b, 2)
            }
            Self::Ack { sequence, offset } => {
                b[1] = 1;
                b[2..4].copy_from_slice(&sequence.to_le_bytes());
                b[4..6].copy_from_slice(&offset.to_le_bytes());
                b[6] = WINDOW;
                (b, 7)
            }
        }
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        match b {
            [VERSION, 0] => Ok(Self::Start),
            [VERSION, 2] => Ok(Self::Probe),
            [VERSION, 1, a, b, c, d, WINDOW] => Ok(Self::Ack {
                sequence: u16::from_le_bytes([*a, *b]),
                offset: u16::from_le_bytes([*c, *d]),
            }),
            _ => Err(Error::Invalid),
        }
    }
}
pub fn encode(kind: u8, body: &[u8], out: &mut [u8]) -> Result<usize, Error> {
    validate(kind, body)?;
    let n = HEADER + body.len();
    if out.len() < n {
        return Err(Error::Length);
    }
    out[..4].copy_from_slice(b"PES1");
    out[4] = VERSION;
    out[5] = kind;
    out[HEADER..n].copy_from_slice(body);
    Ok(n)
}
pub fn decode(b: &[u8]) -> Result<(u8, &[u8]), Error> {
    if b.len() < HEADER || &b[..4] != b"PES1" || b[4] != VERSION {
        return Err(Error::Invalid);
    }
    let body = &b[HEADER..];
    validate(b[5], body)?;
    Ok((b[5], body))
}
fn validate(kind: u8, body: &[u8]) -> Result<(), Error> {
    match kind {
        FILESYSTEM => {
            super::filesystem::Page::decode(body)?;
        }
        BUTTON if body.len() == 1 && body[0] < 4 => {}
        PROBE
            if body.len() == super::filesystem::PAGE_MAX
                && body[..2040].iter().enumerate().all(|(i, b)| *b == i as u8)
                && (23..=128).contains(&u16::from_le_bytes([body[2040], body[2041]]))
                && body[2042] <= WINDOW
                && body[2043] <= 8
                && body[2044] <= 1
                && body[2045..].iter().all(|b| *b == 0) => {}
        _ => return Err(Error::Invalid),
    }
    Ok(())
}
/// One owned ciphertext, four fragment credits, and one cumulative ACK per batch.
/// ACK must match the bytes actually sent; a final ACK releases the record.
pub struct Sender {
    bytes: [u8; CIPHERTEXT_MAX],
    length: usize,
    offset: usize,
    acked: usize,
    sequence: u16,
    credits: u8,
    enabled: bool,
    high_water: u8,
    receipt: Option<(u16, u16)>,
}
impl Default for Sender {
    fn default() -> Self {
        Self::new()
    }
}
impl Sender {
    pub const fn new() -> Self {
        Self {
            bytes: [0; CIPHERTEXT_MAX],
            length: 0,
            offset: 0,
            acked: 0,
            sequence: 0,
            credits: 0,
            enabled: false,
            high_water: 0,
            receipt: None,
        }
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn idle(&self) -> bool {
        self.length == 0
    }
    pub fn reset(&mut self) {
        self.bytes.fill(0);
        self.length = 0;
        self.offset = 0;
        self.acked = 0;
        self.enabled = false;
        self.receipt = None;
        self.credits = 0;
    }
    pub fn start(&mut self) -> Result<(), Error> {
        if self.enabled {
            return Err(Error::Invalid);
        }
        self.enabled = true;
        self.credits = WINDOW;
        Ok(())
    }
    pub fn set(&mut self, sequence: u16, b: &[u8]) -> Result<(), Error> {
        if !self.enabled
            || !self.idle()
            || sequence <= self.sequence
            || b.len() <= 16
            || b.len() > CIPHERTEXT_MAX
        {
            return Err(Error::Invalid);
        }
        self.bytes[..b.len()].copy_from_slice(b);
        self.length = b.len();
        self.offset = 0;
        self.acked = 0;
        self.sequence = sequence;
        Ok(())
    }
    pub fn next_fragment(&mut self) -> Result<Option<[u8; FRAME_LEN]>, Error> {
        if !self.enabled || self.length == 0 || self.credits == 0 || self.offset == self.length {
            return Ok(None);
        }
        let (f, n) = fragment(
            KIND_STREAM,
            self.sequence,
            &self.bytes[..self.length],
            self.offset,
        )?;
        self.offset += n - 8;
        self.credits -= 1;
        self.high_water = self.high_water.max(self.outstanding() as u8);
        Ok(Some(f))
    }
    pub fn ack(&mut self, sequence: u16, offset: u16) -> Result<(), Error> {
        if self.receipt == Some((sequence, offset)) {
            return Ok(());
        }
        let offset = usize::from(offset);
        if sequence == self.sequence && offset == self.acked && offset != 0 {
            return Ok(());
        }
        if self.length == 0
            || sequence != self.sequence
            || offset != self.offset
            || offset <= self.acked
            || (self.credits != 0 && offset != self.length)
        {
            return Err(Error::Invalid);
        }
        self.receipt = Some((sequence, offset as u16));
        self.acked = offset;
        self.credits = WINDOW;
        if offset == self.length {
            self.bytes[..self.length].fill(0);
            self.length = 0;
        }
        Ok(())
    }
    pub fn high_water(&self) -> u8 {
        self.high_water
    }
    pub fn outstanding(&self) -> usize {
        (self.offset - self.acked).div_ceil(PAYLOAD_LEN)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maximum_record_credit_pressure_and_final_receipt() {
        let mut s = Sender::new();
        s.start().unwrap();
        s.set(1, &[42; CIPHERTEXT_MAX]).unwrap();
        let mut offset = 0;
        while !s.idle() {
            let mut count = 0;
            while let Some(f) = s.next_fragment().unwrap() {
                let exact = crate::read_fragment(&f).unwrap();
                offset += exact.len() - 8;
                count += 1;
                assert!(s.outstanding() <= usize::from(WINDOW));
            }
            assert!(count <= WINDOW);
            assert!(s.set(2, &[1; 20]).is_err());
            assert!(s.ack(2, offset as u16).is_err());
            s.ack(1, offset as u16).unwrap();
            assert!(s.ack(1, offset as u16).is_ok());
        }
        assert_eq!(offset, CIPHERTEXT_MAX);
        // Real firmware polls idle senders continuously between records.
        assert!(s.next_fragment().unwrap().is_none());
        s.set(2, &[1; 20]).unwrap();
        assert!(s.ack(1, CIPHERTEXT_MAX as u16).is_ok());
        assert_eq!(s.outstanding(), 0);
        s.reset();
        assert!(s.next_fragment().unwrap().is_none());
        assert!(s.ack(2, 12).is_err());
    }
    #[test]
    fn stream_receiver_rejects_command_lane_and_stale_records() {
        let mut r = crate::Receiver::new_stream();
        let (f, n) = fragment(crate::KIND_RECORD, 1, &[1; 20], 0).unwrap();
        assert!(r.push(&f[..n]).is_err());
        let (f, n) = fragment(KIND_STREAM, 1, &[1; 17], 0).unwrap();
        assert!(crate::Receiver::new().push(&f[..n]).is_err());
        assert!(r.push(&f[..n]).unwrap().is_none());
        let (f, n) = fragment(KIND_STREAM, 1, &[1; 17], 12).unwrap();
        assert!(r.push(&f[..n]).unwrap().is_some());
        assert!(r.push(&f[..n]).is_err());
    }
}
