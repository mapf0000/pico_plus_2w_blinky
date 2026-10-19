//! Bounded adapter between USB relay and the connection-owned BLE stream.
use crate::{
    ble_download_core::Bridge,
    usb::ctrl::{CtrlCommand, try_command},
};
use core::cell::RefCell;
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use embassy_time::{Duration, Instant, Timer};
static STATE: Mutex<CriticalSectionRawMutex, RefCell<Bridge>> =
    Mutex::new(RefCell::new(Bridge::new()));
fn abort(id: u64) {
    if id != 0 {
        let _ = try_command(CtrlCommand::AbortTransfer { transfer_id: id });
    }
}
pub fn activate(owner: u32) {
    reset();
    STATE.lock(|s| s.borrow_mut().activate(owner));
}
pub fn reset() {
    abort(STATE.lock(|s| s.borrow_mut().reset()));
}
pub fn active() -> bool {
    STATE.lock(|s| s.borrow().active())
}
pub fn command(owner: u32, body: &[u8]) -> ble_protocol::Code {
    use ble_protocol::{Code, download::Command};
    match Command::decode(body) {
        Ok(Command::Cancel) => {
            if let Some(id) = STATE.lock(|s| s.borrow_mut().cancel(owner)) {
                abort(id);
                Code::DownloadAccepted
            } else {
                Code::Rejected
            }
        }
        Ok(Command::Session(bytes)) => {
            if !crate::capabilities::host_agent_present() {
                return Code::AgentUnavailable;
            }
            if !STATE.lock(|s| s.borrow_mut().enable(owner)) {
                return Code::Rejected;
            }
            let Ok(payload) = heapless::Vec::from_slice(bytes) else {
                return Code::Rejected;
            };
            if try_command(CtrlCommand::SecureTransfer { payload }).is_ok() {
                Code::DownloadAccepted
            } else {
                Code::Busy
            }
        }
        Err(_) => Code::Rejected,
    }
}
pub fn queue(tag: u8, body: &[u8]) -> bool {
    STATE.lock(|s| s.borrow_mut().enqueue(tag, body)).is_some()
}
pub async fn send(tag: u8, body: &[u8]) -> bool {
    let Some((owner, ticket)) = STATE.lock(|s| s.borrow_mut().enqueue(tag, body)) else {
        return false;
    };
    let at = Instant::now();
    loop {
        if let Some(result) = STATE.lock(|s| s.borrow().status(owner, ticket)) {
            return result;
        }
        if at.elapsed() > Duration::from_secs(60) {
            reset();
            return false;
        }
        Timer::after_millis(5).await;
    }
}
pub fn take(out: &mut [u8; ble_protocol::stream::PLAINTEXT_MAX]) -> Option<usize> {
    STATE.lock(|s| s.borrow_mut().take(out))
}
pub fn ack() {
    STATE.lock(|s| s.borrow_mut().ack());
}
