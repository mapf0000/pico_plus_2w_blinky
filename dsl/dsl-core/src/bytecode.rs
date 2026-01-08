#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

#[cfg(feature = "std")]
use std::vec::Vec;

use crate::limits::MAX_TOTAL_FLAT_OPS;
use crate::{FlatOp, FlatProgram};

pub const MAGIC: [u8; 4] = *b"KBD1";
pub const OP_DELAY: u8 = 0x01;
pub const OP_TAP: u8 = 0x02;
pub const OP_END: u8 = 0xFF;

/// Encode a FlatProgram into compact, versioned bytecode with CRC32 trailer.
pub fn encode(prog: &FlatProgram) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4 + 1 + prog.ops.len() * 4 + 5);
    buf.extend_from_slice(&MAGIC);
    buf.push(0x00); // flags (reserved)

    for op in &prog.ops {
        match *op {
            FlatOp::DelayMs(ms) => {
                buf.push(OP_DELAY);
                write_varu32(&mut buf, ms);
            }
            FlatOp::Tap { usage, mods } => {
                buf.push(OP_TAP);
                buf.push(usage);
                buf.push(mods);
            }
        }
    }
    buf.push(OP_END);

    let crc = crc32(&buf);
    buf.extend_from_slice(&crc.to_le_bytes());
    buf
}

/// Streaming reader for firmware side (no allocation required).
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    crc_range_end: usize, // position of CRC (exclusive)
}

#[derive(Debug)]
pub enum DecodeError {
    BadMagic,
    UnexpectedEof,
    BadVarint,
    BadOpcode,
    BadCrc,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, DecodeError> {
        if data.len() < 4 + 1 + 4 {
            return Err(DecodeError::UnexpectedEof);
        }
        if &data[0..4] != &MAGIC {
            return Err(DecodeError::BadMagic);
        }
        // flags = data[4], ignore for now
        // Find CRC at end (last 4 bytes)
        let crc_range_end = data
            .len()
            .checked_sub(4)
            .ok_or(DecodeError::UnexpectedEof)?;
        Ok(Self {
            data,
            pos: 5,
            crc_range_end,
        })
    }

    pub fn read_u8(&mut self) -> Result<u8, DecodeError> {
        if self.pos >= self.crc_range_end {
            return Err(DecodeError::UnexpectedEof);
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }

    pub fn read_varu32(&mut self) -> Result<u32, DecodeError> {
        let mut result: u32 = 0;
        let mut shift = 0;
        for _ in 0..5 {
            let b = self.read_u8()?;
            result |= ((b & 0x7F) as u32) << shift;
            if (b & 0x80) == 0 {
                return Ok(result);
            }
            shift += 7;
        }
        Err(DecodeError::BadVarint)
    }

    /// After finishing the stream, verify the CRC32 trailer.
    pub fn verify_crc(self) -> Result<(), DecodeError> {
        // Must be positioned exactly at crc_range_end now or later (we ignore extra padding),
        // but we always compute CRC over [0 .. crc_range_end]
        if self.data.len() < self.crc_range_end + 4 {
            return Err(DecodeError::UnexpectedEof);
        }
        let trailer = &self.data[self.crc_range_end..self.crc_range_end + 4];
        let expected = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
        let actual = crc32(&self.data[..self.crc_range_end]);
        if expected == actual {
            Ok(())
        } else {
            Err(DecodeError::BadCrc)
        }
    }
}

// Convenience (host/testing): decode into a FlatProgram (alloc)
pub fn decode_to_flat(data: &[u8]) -> Result<FlatProgram, DecodeError> {
    let mut rd = Reader::new(data)?;
    let mut out = FlatProgram::new();
    loop {
        let op = rd.read_u8()?;
        match op {
            OP_DELAY => {
                let ms = rd.read_varu32()?;
                if ms != 0 {
                    out.ops.push(FlatOp::DelayMs(ms));
                }
            }
            OP_TAP => {
                let usage = rd.read_u8()?;
                let mods = rd.read_u8()?;
                out.ops.push(FlatOp::Tap { usage, mods });
            }
            OP_END => break,
            _ => return Err(DecodeError::BadOpcode),
        }
        if out.ops.len() > MAX_TOTAL_FLAT_OPS {
            return Err(DecodeError::BadOpcode);
        }
    }
    rd.verify_crc()?;
    Ok(out)
}

fn write_varu32(buf: &mut Vec<u8>, mut v: u32) {
    loop {
        let mut b = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        buf.push(b);
        if v == 0 {
            break;
        }
    }
}

// Small, table-free CRC32 (IEEE) for tiny code size.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB88320u32 & mask);
        }
    }
    !crc
}
