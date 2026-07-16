#![cfg(feature = "psram")]

use core::{ptr::NonNull, slice};
use embassy_rp::Peri;
use embassy_rp::psram::{Config as PsramConfig, Psram};
use embassy_rp::qmi_cs1::QmiCs1;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use portable_atomic::{AtomicBool, AtomicU8, Ordering};

// HTTP workers, the singleton WebSocket server, and the transfer pump own
// disjoint regions. PSRAM is a fixed pool here, not a general allocator.
pub const HTTP_BUFFER_COUNT: usize = 3;
pub const HTTP_RX_SIZE: usize = 8 * 1024;
pub const HTTP_TX_SIZE: usize = 32 * 1024;
pub const WEBSOCKET_RX_SIZE: usize = 8 * 1024;
pub const WEBSOCKET_TX_SIZE: usize = 32 * 1024;
pub const TRANSFER_BATCH_SIZE: usize = 1 + transfer_protocol::MAX_SECURE_CHUNK_BATCH_LEN;
const HTTP_RX_TOTAL_SIZE: usize = HTTP_BUFFER_COUNT * HTTP_RX_SIZE;
const HTTP_TX_TOTAL_SIZE: usize = HTTP_BUFFER_COUNT * HTTP_TX_SIZE;
const RESERVED_SIZE: usize = HTTP_RX_TOTAL_SIZE
    + HTTP_TX_TOTAL_SIZE
    + WEBSOCKET_RX_SIZE
    + WEBSOCKET_TX_SIZE
    + TRANSFER_BATCH_SIZE;

static PSRAM_INIT: Mutex<ThreadModeRawMutex, bool> = Mutex::new(false);
static PSRAM_READY: AtomicBool = AtomicBool::new(false);
static HTTP_CLAIMED: AtomicU8 = AtomicU8::new(0);
static WEBSOCKET_CLAIMED: AtomicBool = AtomicBool::new(false);
static TRANSFER_BATCH_CLAIMED: AtomicBool = AtomicBool::new(false);
static mut HTTP_RX: Option<NonNull<u8>> = None;
static mut HTTP_TX: Option<NonNull<u8>> = None;
static mut WEBSOCKET_RX: Option<NonNull<u8>> = None;
static mut WEBSOCKET_TX: Option<NonNull<u8>> = None;
static mut TRANSFER_BATCH: Option<NonNull<u8>> = None;

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
        if total < RESERVED_SIZE {
            log::warn!(
                "psram: not enough size for reserved buffers ({} < {})",
                total,
                RESERVED_SIZE
            );
            *PSRAM_INIT.lock().await = false;
            return;
        }
        let (rx, rest) = all.split_at_mut(HTTP_RX_TOTAL_SIZE);
        let (tx, rest) = rest.split_at_mut(HTTP_TX_TOTAL_SIZE);
        let (websocket_rx, rest) = rest.split_at_mut(WEBSOCKET_RX_SIZE);
        let (websocket_tx, rest) = rest.split_at_mut(WEBSOCKET_TX_SIZE);
        let (transfer_batch, _rest) = rest.split_at_mut(TRANSFER_BATCH_SIZE);
        HTTP_RX = NonNull::new(rx.as_mut_ptr());
        HTTP_TX = NonNull::new(tx.as_mut_ptr());
        WEBSOCKET_RX = NonNull::new(websocket_rx.as_mut_ptr());
        WEBSOCKET_TX = NonNull::new(websocket_tx.as_mut_ptr());
        TRANSFER_BATCH = NonNull::new(transfer_batch.as_mut_ptr());
    }
    HTTP_CLAIMED.store(0, Ordering::Release);
    WEBSOCKET_CLAIMED.store(false, Ordering::Release);
    TRANSFER_BATCH_CLAIMED.store(false, Ordering::Release);
    PSRAM_READY.store(true, Ordering::Release);

    *PSRAM_INIT.lock().await = true;
    log::info!(
        "psram: initialized ({} bytes); http workers={}, rx={} each, tx={} each, ws_rx={}, ws_tx={}, transfer_batch={}",
        total,
        HTTP_BUFFER_COUNT,
        HTTP_RX_SIZE,
        HTTP_TX_SIZE,
        WEBSOCKET_RX_SIZE,
        WEBSOCKET_TX_SIZE,
        TRANSFER_BATCH_SIZE
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

pub fn take_transfer_batch_buffer() -> Option<&'static mut [u8; TRANSFER_BATCH_SIZE]> {
    if TRANSFER_BATCH_CLAIMED.swap(true, Ordering::AcqRel) {
        return None;
    }

    unsafe {
        let Some(ptr) = TRANSFER_BATCH else {
            TRANSFER_BATCH_CLAIMED.store(false, Ordering::Release);
            return None;
        };
        let slice = slice::from_raw_parts_mut(ptr.as_ptr(), TRANSFER_BATCH_SIZE);
        Some(
            slice
                .try_into()
                .expect("PSRAM transfer batch slice uses its declared fixed size"),
        )
    }
}

pub fn take_websocket_buffers() -> Option<(&'static mut [u8], &'static mut [u8])> {
    if WEBSOCKET_CLAIMED.swap(true, Ordering::AcqRel) {
        return None;
    }

    unsafe {
        let Some(rx_ptr) = WEBSOCKET_RX else {
            WEBSOCKET_CLAIMED.store(false, Ordering::Release);
            return None;
        };
        let Some(tx_ptr) = WEBSOCKET_TX else {
            WEBSOCKET_CLAIMED.store(false, Ordering::Release);
            return None;
        };
        Some((
            slice::from_raw_parts_mut(rx_ptr.as_ptr(), WEBSOCKET_RX_SIZE),
            slice::from_raw_parts_mut(tx_ptr.as_ptr(), WEBSOCKET_TX_SIZE),
        ))
    }
}
