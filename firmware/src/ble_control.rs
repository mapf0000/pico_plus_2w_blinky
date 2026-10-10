//! One Bluetooth session and one completion slot, shared with the HID executor.
use crate::usb::hid::{HidResult, HidResultStatus};
use ble_protocol::{Code, ResultValue};
use core::{
    cell::Cell,
    sync::atomic::{AtomicU32, Ordering},
};
use embassy_sync::{
    blocking_mutex::{Mutex, raw::ThreadModeRawMutex},
    signal::Signal,
};

static CURRENT: AtomicU32 = AtomicU32::new(0);
static NEXT: AtomicU32 = AtomicU32::new(0);
static DISPLAYED: AtomicU32 = AtomicU32::new(0);
static PASSKEY: AtomicU32 = AtomicU32::new(0);
pub static DECISION: Signal<ThreadModeRawMutex, bool> = Signal::new();
static RESULT: Mutex<ThreadModeRawMutex, Cell<ResultValue>> = Mutex::new(Cell::new(ResultValue {
    token: 0,
    code: Code::Idle,
}));
static ACTIVE: Mutex<ThreadModeRawMutex, Cell<Option<(script_protocol::EffectId, u16)>>> =
    Mutex::new(Cell::new(None));

pub struct Session(pub u32);
impl Session {
    pub fn new() -> Option<Self> {
        let session = NEXT
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_add(1))
            .ok()?
            .checked_add(1)?;
        CURRENT.store(session, Ordering::SeqCst);
        RESULT.lock(|value| {
            value.set(ResultValue {
                token: 0,
                code: Code::Idle,
            })
        });
        ACTIVE.lock(|value| value.set(None));
        clear_pairing();
        Some(Self(session))
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        CURRENT.store(0, Ordering::SeqCst);
        clear_pairing();
        crate::usb::hid::cancel_companion_session(self.0);
        ACTIVE.lock(|value| value.set(None));
    }
}
pub fn result() -> [u8; 20] {
    RESULT.lock(|value| value.get().encode())
}
pub fn publish(token: u16, code: Code) {
    RESULT.lock(|value| value.set(ResultValue { token, code }));
}
pub fn admitted(id: script_protocol::EffectId, token: u16) {
    ACTIVE.lock(|value| value.set(Some((id, token))));
    publish(token, Code::Accepted);
}
pub fn active(id: script_protocol::EffectId) -> bool {
    ACTIVE.lock(|value| value.get().is_some_and(|(other, _)| other == id))
}
pub async fn deliver(session: u32, result: HidResult) {
    if CURRENT.load(Ordering::SeqCst) != session {
        return;
    }
    let id = script_protocol::EffectId {
        request_id: result.request_id,
        process_id: result.process_id,
        effect_id: result.effect_id,
    };
    let token = ACTIVE.lock(|value| {
        let (other, token) = value.get()?;
        if other != id {
            return None;
        }
        value.set(None);
        Some(token)
    });
    if let Some(token) = token {
        publish(
            token,
            match result.status {
                HidResultStatus::Completed => Code::Completed,
                HidResultStatus::Cancelled => Code::Cancelled,
                HidResultStatus::Rejected => Code::Rejected,
                HidResultStatus::UsbUnavailable => Code::UsbUnavailable,
            },
        );
    }
}
pub fn begin_pairing(code: u32) {
    DISPLAYED.store(0, Ordering::SeqCst);
    DECISION.reset();
    PASSKEY.store(code + 1, Ordering::SeqCst);
}
pub fn pairing_code() -> Option<u32> {
    PASSKEY.load(Ordering::SeqCst).checked_sub(1)
}
pub fn clear_pairing() {
    PASSKEY.store(0, Ordering::SeqCst);
    DECISION.reset();
}

pub fn mark_displayed(code: u32) {
    if pairing_code() == Some(code) {
        DISPLAYED.store(code + 1, Ordering::SeqCst);
    }
}
pub fn pairing_displayed() -> bool {
    let code = PASSKEY.load(Ordering::SeqCst);
    code != 0 && DISPLAYED.load(Ordering::SeqCst) == code
}
