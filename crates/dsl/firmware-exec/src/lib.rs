#![no_std]

use embassy_time::Timer;
use embassy_usb::class::hid::HidWriter as UsbHidWriter;
use usbd_hid::descriptor::KeyboardReport;

const DELAY_MOD_DOWN_MS: u64 = 8;
const DELAY_TAP_HOLD_MS: u64 = 20;

#[derive(Debug)]
pub enum ExecError {
    Decode(bytecode::DecodeError),
    TooLong,
}
impl From<bytecode::DecodeError> for ExecError {
    fn from(e: bytecode::DecodeError) -> Self {
        ExecError::Decode(e)
    }
}

#[inline]
async fn tap_with_mod<'d, D>(w: &mut UsbHidWriter<'d, D, 8>, usage: u8, modifier: u8)
where
    D: embassy_usb::driver::Driver<'d>,
{
    if modifier != 0 {
        let mod_down = KeyboardReport {
            keycodes: [0, 0, 0, 0, 0, 0],
            leds: 0,
            modifier,
            reserved: 0,
        };
        let _ = w.write_serialize(&mod_down).await;
        Timer::after_millis(DELAY_MOD_DOWN_MS).await;
        let both_down = KeyboardReport {
            keycodes: [usage, 0, 0, 0, 0, 0],
            leds: 0,
            modifier,
            reserved: 0,
        };
        let _ = w.write_serialize(&both_down).await;
        Timer::after_millis(DELAY_TAP_HOLD_MS).await;
        let key_up = KeyboardReport {
            keycodes: [0, 0, 0, 0, 0, 0],
            leds: 0,
            modifier,
            reserved: 0,
        };
        let _ = w.write_serialize(&key_up).await;
        Timer::after_millis(DELAY_MOD_DOWN_MS).await;
        let mod_up = KeyboardReport {
            keycodes: [0, 0, 0, 0, 0, 0],
            leds: 0,
            modifier: 0,
            reserved: 0,
        };
        let _ = w.write_serialize(&mod_up).await;
    } else {
        let press = KeyboardReport {
            keycodes: [usage, 0, 0, 0, 0, 0],
            leds: 0,
            modifier: 0,
            reserved: 0,
        };
        let release = KeyboardReport {
            keycodes: [0, 0, 0, 0, 0, 0],
            leds: 0,
            modifier: 0,
            reserved: 0,
        };
        let _ = w.write_serialize(&press).await;
        Timer::after_millis(DELAY_TAP_HOLD_MS).await;
        let _ = w.write_serialize(&release).await;
    }
}

/// Execute bytecode streamed from the frontend.
/// Safe-guards: caps max ops to avoid malicious or corrupt inputs.
pub async fn exec_bytecode<'d, D>(
    w: &mut UsbHidWriter<'d, D, 8>,
    code: &[u8],
) -> Result<(), ExecError>
where
    D: embassy_usb::driver::Driver<'d>,
{
    let mut r = bytecode::Reader::new(code)?;
    let mut decoded_ops = 0usize;

    loop {
        let op = r.read_u8()?;
        match op {
            bytecode::OP_DELAY => {
                let ms = r.read_varu32()?;
                if ms != 0 {
                    Timer::after_millis(ms as u64).await;
                }
            }
            bytecode::OP_TAP => {
                let usage = r.read_u8()?;
                let mods = r.read_u8()?;
                tap_with_mod(w, usage, mods).await;
            }
            bytecode::OP_END => break,
            _ => return Err(ExecError::Decode(bytecode::DecodeError::BadOpcode)),
        }
        decoded_ops += 1;
        if decoded_ops > 20_000 {
            return Err(ExecError::TooLong);
        }
    }
    r.verify_crc()?;
    Ok(())
}

mod bytecode {
    pub const MAGIC: [u8; 4] = *b"KBD1";
    pub const OP_DELAY: u8 = 0x01;
    pub const OP_TAP: u8 = 0x02;
    pub const OP_END: u8 = 0xFF;

    #[derive(Debug)]
    pub enum DecodeError {
        BadMagic,
        UnexpectedEof,
        BadVarint,
        BadOpcode,
        BadCrc,
    }

    pub struct Reader<'a> {
        data: &'a [u8],
        pos: usize,
        crc_range_end: usize,
    }

    impl<'a> Reader<'a> {
        pub fn new(data: &'a [u8]) -> Result<Self, DecodeError> {
            if data.len() < 9 {
                return Err(DecodeError::UnexpectedEof);
            }
            if &data[0..4] != &MAGIC {
                return Err(DecodeError::BadMagic);
            }
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

        pub fn verify_crc(self) -> Result<(), DecodeError> {
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
}
