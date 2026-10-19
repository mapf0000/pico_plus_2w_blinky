//! Bounded native encrypted download bridge. Credential/text consumers remain unavailable.
//! USB chunk acknowledgement waits for the native receiver's final BLE credit.
use heapless::{String, Vec};
pub const TRANSFER_TEXT_MAX: usize = 1024;
pub const TRANSFER_BINARY_MAX: usize = 1 + transfer_protocol::TLV_MAX_PAYLOAD;
#[derive(Debug)]
pub struct Unavailable;
pub fn has_active_client() -> bool {
    crate::ble_download::active()
}
pub fn queue_text(_: String<TRANSFER_TEXT_MAX>) -> Result<(), Unavailable> {
    Err(Unavailable)
}
pub fn queue_binary(bytes: Vec<u8, TRANSFER_BINARY_MAX>) -> Result<(), Unavailable> {
    let (tag, body) = relay_record(&bytes).ok_or(Unavailable)?;
    crate::ble_download::queue(tag, body)
        .then_some(())
        .ok_or(Unavailable)
}
pub async fn send_text(_: String<TRANSFER_TEXT_MAX>) -> Result<(), Unavailable> {
    Err(Unavailable)
}
pub async fn send_binary(bytes: Vec<u8, TRANSFER_BINARY_MAX>) -> Result<(), Unavailable> {
    let (tag, body) = relay_record(&bytes).ok_or(Unavailable)?;
    crate::ble_download::send(tag, body)
        .await
        .then_some(())
        .ok_or(Unavailable)
}
fn relay_record(bytes: &[u8]) -> Option<(u8, &[u8])> {
    let (&kind, body) = bytes.split_first()?;
    let tag = match kind {
        3 => 33,
        4 => 20,
        5 => 21,
        6 => 23,
        _ => return None,
    };
    Some((tag, body))
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
