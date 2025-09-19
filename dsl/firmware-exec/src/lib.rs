#![no_std]

use dsl_core::{ bytecode, KEY_ENTER, MOD_LSHIFT }; // just to show reuse; not strictly needed
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
    fn from(e: bytecode::DecodeError) -> Self { ExecError::Decode(e) }
}

#[inline]
async fn tap_with_mod<'d, D>(w: &mut UsbHidWriter<'d, D, 8>, usage: u8, modifier: u8)
where D: embassy_usb::driver::Driver<'d> {
    if modifier != 0 {
        let mod_down = KeyboardReport { keycodes: [0,0,0,0,0,0], leds: 0, modifier, reserved: 0 };
        let _ = w.write_serialize(&mod_down).await; Timer::after_millis(DELAY_MOD_DOWN_MS).await;
        let both_down = KeyboardReport { keycodes: [usage,0,0,0,0,0], leds: 0, modifier, reserved: 0 };
        let _ = w.write_serialize(&both_down).await; Timer::after_millis(DELAY_TAP_HOLD_MS).await;
        let key_up = KeyboardReport { keycodes: [0,0,0,0,0,0], leds: 0, modifier, reserved: 0 };
        let _ = w.write_serialize(&key_up).await; Timer::after_millis(DELAY_MOD_DOWN_MS).await;
        let mod_up = KeyboardReport { keycodes: [0,0,0,0,0,0], leds: 0, modifier: 0, reserved: 0 };
        let _ = w.write_serialize(&mod_up).await;
    } else {
        let press = KeyboardReport { keycodes: [usage,0,0,0,0,0], leds: 0, modifier: 0, reserved: 0 };
        let release = KeyboardReport { keycodes: [0,0,0,0,0,0], leds: 0, modifier: 0, reserved: 0 };
        let _ = w.write_serialize(&press).await; Timer::after_millis(DELAY_TAP_HOLD_MS).await;
        let _ = w.write_serialize(&release).await;
    }
}

/// Execute bytecode streamed from the frontend.
/// Safe-guards: caps max ops to avoid malicious or corrupt inputs.
pub async fn exec_bytecode<'d, D>(w: &mut UsbHidWriter<'d, D, 8>, code: &[u8]) -> Result<(), ExecError>
where D: embassy_usb::driver::Driver<'d> {
    let mut r = bytecode::Reader::new(code)?;
    let mut decoded_ops = 0usize;

    loop {
        let op = r.read_u8()?;
        match op {
            bytecode::OP_DELAY => {
                let ms = r.read_varu32()?;
                if ms != 0 { Timer::after_millis(ms as u64).await; }
            }
            bytecode::OP_TAP => {
                let usage = r.read_u8()?;
                let mods  = r.read_u8()?;
                tap_with_mod(w, usage, mods).await;
            }
            bytecode::OP_END => break,
            _ => return Err(ExecError::Decode(bytecode::DecodeError::BadOpcode)),
        }
        decoded_ops += 1;
        if decoded_ops > 20_000 { return Err(ExecError::TooLong); }
    }
    r.verify_crc()?;
    Ok(())
}
