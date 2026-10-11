//! Hardware adapters for the connection-owned bounded filesystem/button relay.
use crate::{
    ble_relay_core::Relay,
    usb::ctrl::{CtrlCommand, try_command},
};
use ble_protocol::{filesystem as fs, stream};
use core::cell::RefCell;
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
static STATE: Mutex<CriticalSectionRawMutex, RefCell<Relay>> =
    Mutex::new(RefCell::new(Relay::new()));
fn send_cancel(id: Option<u64>) {
    if let Some(id) = id {
        let _ = try_command(CtrlCommand::CancelDirectoryList { request_id: id });
    }
}
pub fn activate(owner: u32) {
    reset();
    STATE.lock(|s| s.borrow_mut().activate(owner));
}
pub fn reset() {
    send_cancel(STATE.lock(|s| s.borrow_mut().reset()));
}
pub fn list(owner: u32, token: u16, request: fs::Request<'_>) -> ble_protocol::Code {
    use portable_atomic::Ordering;
    if !crate::capabilities::host_agent_present()
        || !crate::usb::ctrl::CTRL_READY.load(Ordering::SeqCst)
    {
        return ble_protocol::Code::AgentUnavailable;
    }
    let Ok(path) = heapless::String::try_from(request.path) else {
        return ble_protocol::Code::Rejected;
    };
    let Ok((usb_id, old)) = STATE.lock(|s| s.borrow_mut().reserve(owner, token, request.id)) else {
        return ble_protocol::Code::Rejected;
    };
    send_cancel(old);
    if try_command(CtrlCommand::ListDirectory {
        request_id: usb_id,
        cursor: request.cursor,
        entry_limit: 64,
        flags: u8::from(request.hidden),
        path,
    })
    .is_err()
    {
        cancel(owner, request.id);
        return ble_protocol::Code::Busy;
    }
    ble_protocol::Code::DirectoryAccepted
}
pub fn cancel(owner: u32, id: u64) {
    send_cancel(STATE.lock(|s| s.borrow_mut().cancel(owner, id)));
}
pub fn page(bytes: &[u8]) -> bool {
    STATE.lock(|s| s.borrow_mut().page(bytes))
}
pub fn button(button: u8) {
    STATE.lock(|s| s.borrow_mut().button(button));
}
pub fn take(out: &mut [u8; stream::PLAINTEXT_MAX]) -> Option<usize> {
    STATE.lock(|s| s.borrow_mut().take(out))
}

pub fn high_water() -> [u8; 2] {
    STATE.lock(|s| s.borrow().high_water())
}

const _: () = assert!(fs::VERSION == crate::capabilities::FILESYSTEM_PROTOCOL_VERSION);
const _: () = assert!(fs::PATH_MAX == crate::usb::ctrl::MAX_TRANSFER_PATH_LEN);
