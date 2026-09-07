//! Bounded KBD1 encoding and decoding.

#[cfg(all(feature = "alloc", not(feature = "std")))]
use alloc::vec::Vec;

#[cfg(all(feature = "alloc", feature = "std"))]
use std::vec::Vec;

#[cfg(feature = "alloc")]
use crate::{FlatOp, FlatProgram, FlatProgramError, Mods, Usage};
pub use bytecode_constants::MAX_BYTECODE;
use bytecode_constants::{MAX_BYTECODE_OPS, MAX_DEVICE_DELAY_MS, MAX_EFFECT_DELAY_TOTAL_MS};

pub const MAGIC: [u8; 4] = *b"KBD1";
pub const OP_DELAY: u8 = 0x01;
pub const OP_TAP: u8 = 0x02;
pub const OP_END: u8 = 0xFF;

/// Encode a FlatProgram into compact, versioned bytecode with CRC32 trailer.
/// Normalizes and validates flat op invariants (no zero delays, no adjacent delays, ops cap).
#[cfg(feature = "alloc")]
pub fn encode(prog: &FlatProgram) -> Result<Vec<u8>, FlatProgramError> {
    let mut normalized = prog.clone();
    normalized.normalize_in_place()?;
    let mut buf = Vec::with_capacity(4 + 1 + normalized.ops.len() * 4 + 5);
    buf.extend_from_slice(&MAGIC);
    buf.push(0x00); // flags (reserved)

    for op in &normalized.ops {
        match *op {
            FlatOp::DelayMs(ms) => {
                buf.push(OP_DELAY);
                write_varu32(&mut buf, ms);
            }
            FlatOp::Tap { usage, mods } => {
                buf.push(OP_TAP);
                buf.push(usage.bits());
                buf.push(mods.bits());
            }
        }
    }
    buf.push(OP_END);

    let crc = crc32(&buf);
    buf.extend_from_slice(&crc.to_le_bytes());
    Ok(buf)
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
    BadFlags,
    UnexpectedEof,
    BadVarint,
    NonCanonicalVarint,
    BadOpcode,
    BadUsage,
    ZeroDelay,
    DelayTooLong,
    TotalDelayTooLong,
    TooManyOps,
    TrailingData,
    BytecodeTooLong,
    BadCrc,
}

/// Strictly validate an exact KBD1 program before execution or transport.
pub fn validate(data: &[u8]) -> Result<(), DecodeError> {
    if data.len() > MAX_BYTECODE {
        return Err(DecodeError::BytecodeTooLong);
    }
    let mut reader = Reader::new(data)?;
    if data[4] != 0 {
        return Err(DecodeError::BadFlags);
    }
    reader.verify_crc()?;

    let mut operations = 0usize;
    let mut total_delay = 0u64;
    loop {
        match reader.read_u8()? {
            OP_DELAY => {
                let milliseconds = reader.read_varu32()?;
                if milliseconds == 0 {
                    return Err(DecodeError::ZeroDelay);
                }
                if milliseconds > MAX_DEVICE_DELAY_MS {
                    return Err(DecodeError::DelayTooLong);
                }
                total_delay += milliseconds as u64;
                if total_delay > MAX_EFFECT_DELAY_TOTAL_MS {
                    return Err(DecodeError::TotalDelayTooLong);
                }
            }
            OP_TAP => {
                let usage = reader.read_u8()?;
                let _modifiers = reader.read_u8()?;
                if !(0x04..=0x73).contains(&usage) {
                    return Err(DecodeError::BadUsage);
                }
            }
            OP_END => {
                if reader.pos != reader.crc_range_end {
                    return Err(DecodeError::TrailingData);
                }
                return Ok(());
            }
            _ => return Err(DecodeError::BadOpcode),
        }
        operations += 1;
        if operations > MAX_BYTECODE_OPS {
            return Err(DecodeError::TooManyOps);
        }
    }
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, DecodeError> {
        if data.len() < 4 + 1 + 1 + 4 {
            return Err(DecodeError::UnexpectedEof);
        }
        if data[0..4] != MAGIC {
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
        for index in 0..5 {
            let b = self.read_u8()?;
            if index == 4 && (b & 0xf0) != 0 {
                return Err(DecodeError::BadVarint);
            }
            result |= ((b & 0x7F) as u32) << shift;
            if (b & 0x80) == 0 {
                if index > 0 && result < (1u32 << (index * 7)) {
                    return Err(DecodeError::NonCanonicalVarint);
                }
                return Ok(result);
            }
            shift += 7;
        }
        Err(DecodeError::BadVarint)
    }

    /// After finishing the stream, verify the CRC32 trailer.
    pub fn verify_crc(&self) -> Result<(), DecodeError> {
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
#[cfg(feature = "alloc")]
pub fn decode_to_flat(data: &[u8]) -> Result<FlatProgram, DecodeError> {
    validate(data)?;
    let mut rd = Reader::new(data)?;
    let mut out = FlatProgram::new();
    loop {
        let op = rd.read_u8()?;
        match op {
            OP_DELAY => {
                let ms = rd.read_varu32()?;
                out.push_delay(ms).map_err(|_| DecodeError::TooManyOps)?;
            }
            OP_TAP => {
                let usage = Usage::from_u8(rd.read_u8()?);
                let mods = Mods::from_u8(rd.read_u8()?);
                out.push_tap(usage, mods)
                    .map_err(|_| DecodeError::TooManyOps)?;
            }
            OP_END => break,
            _ => return Err(DecodeError::BadOpcode),
        }
    }
    Ok(out)
}

#[cfg(feature = "alloc")]
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
