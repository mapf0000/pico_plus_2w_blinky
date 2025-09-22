#![cfg_attr(not(test), no_std)]

// Re-export only the modules that are platform-agnostic so we can run
// unit tests on a host (macOS) with `cargo test --lib`.
pub mod host;
pub mod script_dsl;
pub mod scripts;

// Provide `crate::usb::keyboard` for the library without pulling in
// device-specific USB modules (`hid`, `usb_supervisor`).
#[path = "usb/keyboard.rs"]
pub(crate) mod usb_keyboard;
mod usb {
    pub(crate) use crate::usb_keyboard as keyboard;
}
