//! Strict, cancellable firmware-side KBD1 execution.
#![no_std]

#[cfg(test)]
extern crate std;

use embassy_time::Timer;
use embassy_usb::{class::hid::HidWriter as UsbHidWriter, driver::EndpointError};
pub use keyboard_core::bytecode;
use usbd_hid::descriptor::KeyboardReport;

const DELAY_MOD_DOWN_MS: u64 = 8;
const DELAY_TAP_HOLD_MS: u64 = 20;

#[derive(Debug)]
pub enum ExecError {
    Decode(bytecode::DecodeError),
    Usb(EndpointError),
}
impl From<bytecode::DecodeError> for ExecError {
    fn from(e: bytecode::DecodeError) -> Self {
        ExecError::Decode(e)
    }
}
impl From<EndpointError> for ExecError {
    fn from(error: EndpointError) -> Self {
        ExecError::Usb(error)
    }
}

/// Validate the complete program before any HID report is emitted.
pub fn validate_bytecode(code: &[u8]) -> Result<(), ExecError> {
    bytecode::validate(code).map_err(Into::into)
}

/// Best-effort release of every keyboard key and modifier.
pub async fn release_all<'d, D>(w: &mut UsbHidWriter<'d, D, 8>)
where
    D: embassy_usb::driver::Driver<'d>,
{
    let release = KeyboardReport {
        keycodes: [0, 0, 0, 0, 0, 0],
        leds: 0,
        modifier: 0,
        reserved: 0,
    };
    let _ = w.write_serialize(&release).await;
}

#[inline]
async fn tap_with_mod<'d, D>(
    w: &mut UsbHidWriter<'d, D, 8>,
    usage: u8,
    modifier: u8,
) -> Result<(), EndpointError>
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
        w.write_serialize(&mod_down).await?;
        Timer::after_millis(DELAY_MOD_DOWN_MS).await;
        let both_down = KeyboardReport {
            keycodes: [usage, 0, 0, 0, 0, 0],
            leds: 0,
            modifier,
            reserved: 0,
        };
        w.write_serialize(&both_down).await?;
        Timer::after_millis(DELAY_TAP_HOLD_MS).await;
        let key_up = KeyboardReport {
            keycodes: [0, 0, 0, 0, 0, 0],
            leds: 0,
            modifier,
            reserved: 0,
        };
        w.write_serialize(&key_up).await?;
        Timer::after_millis(DELAY_MOD_DOWN_MS).await;
        let mod_up = KeyboardReport {
            keycodes: [0, 0, 0, 0, 0, 0],
            leds: 0,
            modifier: 0,
            reserved: 0,
        };
        w.write_serialize(&mod_up).await?;
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
        w.write_serialize(&press).await?;
        Timer::after_millis(DELAY_TAP_HOLD_MS).await;
        w.write_serialize(&release).await?;
    }
    Ok(())
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
    validate_bytecode(code)?;
    let mut r = bytecode::Reader::new(code)?;

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
                tap_with_mod(w, usage, mods).await?;
            }
            bytecode::OP_END => break,
            _ => return Err(ExecError::Decode(bytecode::DecodeError::BadOpcode)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{bytecode, validate_bytecode};

    fn program(body: &[u8], flags: u8) -> std::vec::Vec<u8> {
        let mut bytes = std::vec::Vec::from(bytecode::MAGIC);
        bytes.push(flags);
        bytes.extend_from_slice(body);
        let crc = crc32(&bytes);
        bytes.extend_from_slice(&crc.to_le_bytes());
        bytes
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &byte in bytes {
            crc ^= byte as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }

    #[test]
    fn accepts_a_strict_program() {
        let bytes = program(
            &[
                bytecode::OP_TAP,
                0x04,
                0x02,
                bytecode::OP_DELAY,
                0x0a,
                bytecode::OP_END,
            ],
            0,
        );
        assert!(validate_bytecode(&bytes).is_ok());
    }

    #[test]
    fn rejects_crc_flags_trailing_and_noncanonical_delay() {
        let bad_flags = program(&[bytecode::OP_END], 1);
        assert!(matches!(
            bytecode::validate(&bad_flags),
            Err(bytecode::DecodeError::BadFlags)
        ));

        let trailing = program(&[bytecode::OP_END, 0], 0);
        assert!(matches!(
            bytecode::validate(&trailing),
            Err(bytecode::DecodeError::TrailingData)
        ));

        let noncanonical = program(&[bytecode::OP_DELAY, 0x81, 0x00, bytecode::OP_END], 0);
        assert!(matches!(
            bytecode::validate(&noncanonical),
            Err(bytecode::DecodeError::NonCanonicalVarint)
        ));

        let mut bad_crc = program(&[bytecode::OP_END], 0);
        *bad_crc.last_mut().unwrap() ^= 1;
        assert!(matches!(
            bytecode::validate(&bad_crc),
            Err(bytecode::DecodeError::BadCrc)
        ));
    }

    #[test]
    fn rejects_opcode_usage_and_delay_limits() {
        for (body, expected) in [
            (
                std::vec![0x77, bytecode::OP_END],
                bytecode::DecodeError::BadOpcode,
            ),
            (
                std::vec![bytecode::OP_TAP, 0x03, 0, bytecode::OP_END],
                bytecode::DecodeError::BadUsage,
            ),
            (
                std::vec![bytecode::OP_DELAY, 0, bytecode::OP_END],
                bytecode::DecodeError::ZeroDelay,
            ),
            (
                std::vec![bytecode::OP_DELAY, 0x89, 0x27, bytecode::OP_END],
                bytecode::DecodeError::DelayTooLong,
            ),
        ] {
            let bytes = program(&body, 0);
            assert_eq!(
                core::mem::discriminant(&bytecode::validate(&bytes).unwrap_err()),
                core::mem::discriminant(&expected)
            );
        }

        let mut delays = std::vec::Vec::new();
        for _ in 0..61 {
            delays.extend_from_slice(&[bytecode::OP_DELAY, 0x88, 0x27]);
        }
        delays.push(bytecode::OP_END);
        let bytes = program(&delays, 0);
        assert!(matches!(
            bytecode::validate(&bytes),
            Err(bytecode::DecodeError::TotalDelayTooLong)
        ));

        let oversized = std::vec![0; bytecode::MAX_BYTECODE + 1];
        assert!(matches!(
            bytecode::validate(&oversized),
            Err(bytecode::DecodeError::BytecodeTooLong)
        ));
    }
}
