use core::sync::atomic::{AtomicBool, Ordering};

use embassy_executor::Spawner;
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, mutex::Mutex, signal::Signal};

// Reuse global flags from main
// Global USB enabled flag, now lives in the usb module
pub static USB_ENABLED: AtomicBool = AtomicBool::new(false);

// Signals for triggering start/stop of the single USB session.
pub(crate) static START_REQ: Signal<ThreadModeRawMutex, ()> = Signal::new();
static USB_CANCEL: Signal<ThreadModeRawMutex, u64> = Signal::new();
static USB_STOPPED: Signal<ThreadModeRawMutex, ()> = Signal::new();
static STARTED: AtomicBool = AtomicBool::new(false);
/// Serializes USB lifecycle requests and identity flash transactions.
pub(crate) static LIFECYCLE: Mutex<ThreadModeRawMutex, ()> = Mutex::new(());

pub fn init(spawner: Spawner) {
    let _ = crate::log_spawn(
        &spawner,
        "hid_completion",
        crate::usb::hid::completion_task(),
    );
    // Spawn a single USB task that will wait for START_REQ, then run sessions on demand.
    let _ = crate::log_spawn(
        &spawner,
        "usb_task",
        crate::usb::task::usb_task(&USB_CANCEL, &START_REQ),
    );
    // Auto-enable so CDC logs come up without an external trigger. The task still
    // waits for a request between sessions so it yields after USB is stopped.
    USB_ENABLED.store(true, Ordering::SeqCst);
    START_REQ.signal(());
}

pub async fn start() -> Result<(), ()> {
    let _lifecycle = LIFECYCLE.lock().await;
    if STARTED.load(Ordering::SeqCst) && !USB_ENABLED.load(Ordering::SeqCst) {
        return Err(()); // A timed-out shutdown is still releasing the session.
    }
    if USB_ENABLED.swap(true, Ordering::SeqCst) {
        return Ok(()); // already enabled or in-flight
    }
    START_REQ.signal(());
    Ok(())
}

pub async fn stop(detach_ms: u64) -> Result<(), ()> {
    let _lifecycle = LIFECYCLE.lock().await;
    USB_ENABLED.store(false, Ordering::SeqCst);
    if STARTED.load(Ordering::SeqCst) {
        USB_STOPPED.reset();
        USB_CANCEL.signal(detach_ms);
        USB_STOPPED.wait().await;
    }
    Ok(())
}

pub fn is_running() -> bool {
    STARTED.load(Ordering::SeqCst)
}

pub(crate) fn notify_started() {
    STARTED.store(true, Ordering::SeqCst);
}

pub(crate) fn notify_stopped() {
    STARTED.store(false, Ordering::SeqCst);
    USB_ENABLED.store(false, Ordering::SeqCst);
    USB_STOPPED.signal(());
}

// runner_task removed; usb_task spawned directly and waits on START_REQ.
