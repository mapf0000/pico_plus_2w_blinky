#![cfg(feature = "psram")]

use core::{ptr::NonNull, slice};
use embassy_rp::Peri;
use embassy_rp::psram::{Config as PsramConfig, Psram};
use embassy_rp::qmi_cs1::QmiCs1;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use portable_atomic::{AtomicBool, AtomicU8, Ordering};

// Each HTTP worker owns disjoint TCP buffers. The larger transmit window lets
// batched transfer messages remain in flight across Wi-Fi/TCP round trips.
pub const HTTP_BUFFER_COUNT: usize = 4;
pub const HTTP_RX_SIZE: usize = 8 * 1024;
pub const HTTP_TX_SIZE: usize = 32 * 1024;
const HTTP_RX_TOTAL_SIZE: usize = HTTP_BUFFER_COUNT * HTTP_RX_SIZE;
const HTTP_TX_TOTAL_SIZE: usize = HTTP_BUFFER_COUNT * HTTP_TX_SIZE;
const HTTP_TOTAL_SIZE: usize = HTTP_RX_TOTAL_SIZE + HTTP_TX_TOTAL_SIZE;

static PSRAM_INIT: Mutex<ThreadModeRawMutex, bool> = Mutex::new(false);
static PSRAM_READY: AtomicBool = AtomicBool::new(false);
static HTTP_CLAIMED: AtomicU8 = AtomicU8::new(0);
static mut HTTP_RX: Option<NonNull<u8>> = None;
static mut HTTP_TX: Option<NonNull<u8>> = None;

pub async fn init(
    qmi_cs1: Peri<'static, embassy_rp::peripherals::QMI_CS1>,
    cs_pin: Peri<'static, embassy_rp::peripherals::PIN_0>,
) {
    // Avoid re-init if already done
    if *PSRAM_INIT.lock().await {
        return;
    }

    // On Pimoroni Pico Plus 2 W, PSRAM (APS6404) is on QMI CS1.
    // CS pin is board-specific; GP0 is a common choice. Adjust if needed.
    let qmi = QmiCs1::new(qmi_cs1, cs_pin);
    let cfg = PsramConfig::aps6404l();
    let Ok(psram) = Psram::new(qmi, cfg) else {
        log::warn!("psram: not detected; continuing without PSRAM buffers");
        *PSRAM_INIT.lock().await = false;
        return;
    };
    let total = psram.size();
    let base = psram.base_address();
    unsafe {
        // Create a slice covering the entire PSRAM; then split for our buffers
        let all: &'static mut [u8] = slice::from_raw_parts_mut(base, total);
        if total < HTTP_TOTAL_SIZE {
            log::warn!(
                "psram: not enough size for http buffers ({} < {})",
                total,
                HTTP_TOTAL_SIZE
            );
            *PSRAM_INIT.lock().await = false;
            return;
        }
        let (rx, rest) = all.split_at_mut(HTTP_RX_TOTAL_SIZE);
        let (tx, _rest2) = rest.split_at_mut(HTTP_TX_TOTAL_SIZE);
        HTTP_RX = NonNull::new(rx.as_mut_ptr());
        HTTP_TX = NonNull::new(tx.as_mut_ptr());
    }
    HTTP_CLAIMED.store(0, Ordering::Release);
    PSRAM_READY.store(true, Ordering::Release);

    *PSRAM_INIT.lock().await = true;
    log::info!(
        "psram: initialized ({} bytes); http workers={}, rx={} each, tx={} each",
        total,
        HTTP_BUFFER_COUNT,
        HTTP_RX_SIZE,
        HTTP_TX_SIZE
    );
}

pub fn is_available() -> bool {
    PSRAM_READY.load(Ordering::Acquire)
}

pub fn http_buffers(worker_id: usize) -> Option<(&'static mut [u8], &'static mut [u8])> {
    if worker_id >= HTTP_BUFFER_COUNT {
        return None;
    }

    unsafe {
        let rx_ptr = HTTP_RX?;
        let tx_ptr = HTTP_TX?;
        let worker_bit = 1u8 << worker_id;
        if HTTP_CLAIMED.fetch_or(worker_bit, Ordering::AcqRel) & worker_bit != 0 {
            return None;
        }

        let rx =
            slice::from_raw_parts_mut(rx_ptr.as_ptr().add(worker_id * HTTP_RX_SIZE), HTTP_RX_SIZE);
        let tx =
            slice::from_raw_parts_mut(tx_ptr.as_ptr().add(worker_id * HTTP_TX_SIZE), HTTP_TX_SIZE);
        Some((rx, tx))
    }
}
