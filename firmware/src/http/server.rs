use embassy_net as net;
use embassy_time as embassy_time_legacy;
use portable_atomic::{AtomicU8, Ordering};

const SERVER_PORT: u16 = 80;

/// Three workers serve independent frontend asset requests. WebSocket traffic
/// has two acceptors and its own pair of buffer slots.
pub const WEB_TASK_POOL_SIZE: usize = 3;
pub(super) const WEBSOCKET_BUFFER_SLOT_START: usize = WEB_TASK_POOL_SIZE;
const SERVER_BUFFER_COUNT: usize =
    WEB_TASK_POOL_SIZE + super::websocket_server::WEBSOCKET_TASK_POOL_SIZE;

type TimerDuration = embassy_time_legacy::Duration;

const SRAM_RX_BUF_SIZE: usize = 4096;
const SRAM_TX_BUF_SIZE: usize = 4096;
#[cfg(feature = "psram")]
const USE_PSRAM_HTTP_SOCKET_BUFFERS: bool = true;
// Request parse buffer (headers + small body staging).
const REQ_BUF_SIZE: usize = 4096;

// All server buffers are static so neither HTTP nor WebSocket futures retain
// large arrays. Slot ownership is claimed once at startup and never released.
static mut SRAM_RX_BUFS: [[u8; SRAM_RX_BUF_SIZE]; SERVER_BUFFER_COUNT] =
    [[0; SRAM_RX_BUF_SIZE]; SERVER_BUFFER_COUNT];
static mut SRAM_TX_BUFS: [[u8; SRAM_TX_BUF_SIZE]; SERVER_BUFFER_COUNT] =
    [[0; SRAM_TX_BUF_SIZE]; SERVER_BUFFER_COUNT];
static mut HTTP_REQ_BUFS: [[u8; REQ_BUF_SIZE]; SERVER_BUFFER_COUNT] =
    [[0; REQ_BUF_SIZE]; SERVER_BUFFER_COUNT];
static SRAM_BUFFER_CLAIMED: AtomicU8 = AtomicU8::new(0);

pub(super) fn take_sram_buffers(
    slot: usize,
) -> Option<(&'static mut [u8], &'static mut [u8], &'static mut [u8])> {
    if slot >= SERVER_BUFFER_COUNT {
        return None;
    }
    let slot_bit = 1u8 << slot;
    if SRAM_BUFFER_CLAIMED.fetch_or(slot_bit, Ordering::AcqRel) & slot_bit != 0 {
        return None;
    }

    unsafe {
        Some((
            &mut SRAM_RX_BUFS[slot],
            &mut SRAM_TX_BUFS[slot],
            &mut HTTP_REQ_BUFS[slot],
        ))
    }
}

/// Spawn the HTTP worker pool (call this from your init).
pub fn spawn_http_server_pool(spawner: &embassy_executor::Spawner, stack: net::Stack<'static>) {
    for id in 0..WEB_TASK_POOL_SIZE {
        if crate::log_spawn(spawner, "http::server_task", server_task(id, stack)) {
            log::info!("http: spawned worker {id} (port {SERVER_PORT})");
        }
    }
}

/// Pooled HTTP server task (embassy-net Stack is Copy — pass by value).
#[embassy_executor::task(pool_size = WEB_TASK_POOL_SIZE)]
pub async fn server_task(id: usize, stack: net::Stack<'static>) -> ! {
    log::info!("http[{id}]: listening on port {}", SERVER_PORT);

    // Build the router via macro (avoids opaque inner type hassles).
    let app = crate::http::routes::app_router!();

    // Asset responses close their TCP connection. With a small fixed worker
    // pool this prevents stale browser keep-alives from blocking a refresh.
    let cfg = picoserve::Config::new(picoserve::Timeouts {
        start_read_request: TimerDuration::from_secs(5),
        persistent_start_read_request: TimerDuration::from_secs(3),
        read_request: TimerDuration::from_secs(2),
        // The lazily requested RustPython runtime is ~3.7 MiB compressed.
        // Allow slow AP clients to finish that single bounded response.
        write: TimerDuration::from_secs(30),
    })
    .close_connection_after_response();

    let (sram_rx, sram_tx, http_buf) =
        take_sram_buffers(id).expect("each HTTP worker has one statically assigned buffer slot");

    // --- TCP buffer selection per worker ---
    #[cfg(feature = "psram")]
    let (rx_buf, tx_buf): (&mut [u8], &mut [u8]) = {
        let psram_buffers = if USE_PSRAM_HTTP_SOCKET_BUFFERS {
            crate::psram_pool::http_buffers(id)
        } else {
            None
        };
        match psram_buffers {
            Some((rx, tx)) => (rx, tx),
            None => {
                log::info!("http[{id}]: using SRAM socket buffers");
                (sram_rx, sram_tx)
            }
        }
    };

    #[cfg(not(feature = "psram"))]
    let (rx_buf, tx_buf): (&mut [u8], &mut [u8]) = (sram_rx, sram_tx);

    log::info!(
        "http[{id}]: buffers ready (rx={}, tx={}, req={})",
        rx_buf.len(),
        tx_buf.len(),
        REQ_BUF_SIZE
    );

    // Serve forever (never returns on current picoserve versions)
    picoserve::Server::new(&app, &cfg, http_buf)
        .listen_and_serve("http", stack, SERVER_PORT, rx_buf, tx_buf)
        .await
        .into_never()
}

#[cfg(feature = "psram")]
const _: () = assert!(WEB_TASK_POOL_SIZE == crate::psram_pool::HTTP_BUFFER_COUNT);
const _: () = assert!(
    WEBSOCKET_BUFFER_SLOT_START + super::websocket_server::WEBSOCKET_TASK_POOL_SIZE
        == SERVER_BUFFER_COUNT
);
