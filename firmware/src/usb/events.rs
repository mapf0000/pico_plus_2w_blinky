//! Retired browser data plane. Native bulk/credential consumers are not implemented.
//! Rejecting delivery is important: secure transfer chunks must never be ACKed
//! as if a receiver had consumed them.
use heapless::{String, Vec};
pub const TRANSFER_TEXT_MAX: usize = 1024;
pub const TRANSFER_BINARY_MAX: usize = 1 + transfer_protocol::TLV_MAX_PAYLOAD;
pub const WS_BINARY_KIND_FILESYSTEM: u8 = 2;
#[derive(Debug)]
pub struct Unavailable;
pub fn has_active_client() -> bool {
    false
}
pub fn queue_text(_: String<TRANSFER_TEXT_MAX>) -> Result<(), Unavailable> {
    Err(Unavailable)
}
pub fn queue_binary(_: Vec<u8, TRANSFER_BINARY_MAX>) -> Result<(), Unavailable> {
    Err(Unavailable)
}
pub async fn send_text(_: String<TRANSFER_TEXT_MAX>) -> Result<(), Unavailable> {
    Err(Unavailable)
}
pub async fn send_binary(_: Vec<u8, TRANSFER_BINARY_MAX>) -> Result<(), Unavailable> {
    Err(Unavailable)
}
pub fn escape_json_str(s: &str) -> String<512> {
    use core::fmt::Write;
    let mut out = String::new();
    for ch in s.chars() {
        match ch {
            '"' => {
                let _ = out.push_str("\\\"");
            }
            '\\' => {
                let _ = out.push_str("\\\\");
            }
            '\n' => {
                let _ = out.push_str("\\n");
            }
            '\r' => {
                let _ = out.push_str("\\r");
            }
            '\t' => {
                let _ = out.push_str("\\t");
            }
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => {
                let _ = out.push(c);
            }
        }
    }
    out
}
