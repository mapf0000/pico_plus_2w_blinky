use core::sync::atomic::{AtomicBool, Ordering};

use embassy_executor::Spawner;
use embassy_rp::peripherals::USB;
use embassy_rp::usb::Driver as UsbDriver;
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, signal::Signal};

// Reuse global flags from main
// Global USB enabled flag, now lives in the usb module
pub static USB_ENABLED: AtomicBool = AtomicBool::new(false);

// Signals for triggering start/stop of the single USB session.
pub(crate) static START_REQ: Signal<ThreadModeRawMutex, bool> = Signal::new();
static USB_CANCEL: Signal<ThreadModeRawMutex, ()> = Signal::new();
static USB_STOPPED: Signal<ThreadModeRawMutex, ()> = Signal::new();
static STARTED: AtomicBool = AtomicBool::new(false);

pub fn init(driver: UsbDriver<'static, USB>, spawner: Spawner) {
    // Spawn a single USB task that will wait for START_REQ, then run until canceled.
    let _ = crate::log_spawn(
        "usb_task",
        spawner.spawn(crate::usb::task::usb_task(
            driver,
            &USB_CANCEL,
            &START_REQ,
            false,
        )),
    );
}

pub async fn start(run_mac_assistant: bool) -> Result<(), ()> {
    if STARTED.swap(true, Ordering::SeqCst) {
        return Ok(()); // already started
    }
    USB_ENABLED.store(true, Ordering::SeqCst);
    START_REQ.signal(run_mac_assistant);
    Ok(())
}

pub async fn stop(_detach_ms: u64) -> Result<(), ()> {
    USB_CANCEL.signal(());
    // Wait until the task confirms it stopped
    USB_STOPPED.wait().await;
    USB_ENABLED.store(false, Ordering::SeqCst);
    Ok(())
}

pub fn is_running() -> bool {
    STARTED.load(Ordering::SeqCst)
}

pub(crate) fn notify_stopped() {
    STARTED.store(false, Ordering::SeqCst);
    USB_STOPPED.signal(());
}

// runner_task removed; usb_task spawned directly and waits on START_REQ.
