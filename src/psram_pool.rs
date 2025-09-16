#![cfg(feature = "psram")]

use core::slice;
use embassy_rp::Peripherals;
use embassy_rp::psram::{Config as PsramConfig, Psram};
use embassy_rp::qmi_cs1::QmiCs1;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;

// Allocate modest buffers to start; can be tuned later.
pub const HTTP_RX_SIZE: usize = 8 * 1024;
pub const HTTP_TX_SIZE: usize = 4 * 1024;

static PSRAM_INIT: Mutex<ThreadModeRawMutex, bool> = Mutex::new(false);
static mut HTTP_RX: Option<&'static mut [u8]> = None;
static mut HTTP_TX: Option<&'static mut [u8]> = None;

pub async fn init(p: &Peripherals) {
    // Avoid re-init if already done
    if *PSRAM_INIT.lock().await {
        return;
    }

    // On Pimoroni Pico Plus 2 W, PSRAM (APS6404) is on QMI CS1.
    // CS pin is board-specific; GP0 is a common choice. Adjust if needed.
    let qmi = QmiCs1::new(p.QMI_CS1, p.PIN_0);
    let cfg = PsramConfig::aps6404l();
    let Ok(psram) = Psram::new(qmi, cfg) else {
        log::warn!("psram: not detected; continuing without PSRAM buffers");
        *PSRAM_INIT.lock().await = false;
        return;
    };
    let total = psram.size() as usize;
    let base = psram.base_address();
    unsafe {
        // Create a slice covering the entire PSRAM; then split for our buffers
        let all: &'static mut [u8] = slice::from_raw_parts_mut(base, total);
        if total < HTTP_RX_SIZE + HTTP_TX_SIZE {
            log::warn!(
                "psram: not enough size for http buffers ({} < {})",
                total,
                HTTP_RX_SIZE + HTTP_TX_SIZE
            );
            *PSRAM_INIT.lock().await = false;
            return;
        }
        let (rx, rest) = all.split_at_mut(HTTP_RX_SIZE);
        let (tx, _rest2) = rest.split_at_mut(HTTP_TX_SIZE);
        HTTP_RX = Some(rx);
        HTTP_TX = Some(tx);
    }

    *PSRAM_INIT.lock().await = true;
    log::info!(
        "psram: initialized ({} bytes); http rx={}, tx={}",
        total,
        HTTP_RX_SIZE,
        HTTP_TX_SIZE
    );
}

pub fn http_buffers() -> Option<(&'static mut [u8], &'static mut [u8])> {
    unsafe {
        match (HTTP_RX.as_deref_mut(), HTTP_TX.as_deref_mut()) {
            (Some(rx), Some(tx)) => Some((rx, tx)),
            _ => None,
        }
    }
}
