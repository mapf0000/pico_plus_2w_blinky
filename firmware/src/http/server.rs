use embassy_net as net;
use embassy_time as embassy_time_legacy;

const SERVER_PORT: u16 = 80;

/// Tune this to match your StackResources<SOCK> budget.
pub const WEB_TASK_POOL_SIZE: usize = 4;

type TimerDuration = embassy_time_legacy::Duration;

// Centralized HTTP sizes and limits for maintainability
#[cfg(not(feature = "psram"))]
const RX_BUF_SIZE: usize = 4096;
#[cfg(not(feature = "psram"))]
const TX_BUF_SIZE: usize = 4096;
#[cfg(feature = "psram")]
const FALLBACK_RX_BUF_SIZE: usize = 4096;
#[cfg(feature = "psram")]
const FALLBACK_TX_BUF_SIZE: usize = 4096;
// Request parse buffer (headers + small body staging).
const REQ_BUF_SIZE: usize = 4096;

// For non-PSRAM builds, keep HTTP buffers out of the task stack to avoid overflows.
#[cfg(not(feature = "psram"))]
static mut RX_BUFS: [[u8; RX_BUF_SIZE]; WEB_TASK_POOL_SIZE] =
    [[0; RX_BUF_SIZE]; WEB_TASK_POOL_SIZE];
#[cfg(not(feature = "psram"))]
static mut TX_BUFS: [[u8; TX_BUF_SIZE]; WEB_TASK_POOL_SIZE] =
    [[0; TX_BUF_SIZE]; WEB_TASK_POOL_SIZE];
#[cfg(not(feature = "psram"))]
static mut HTTP_REQ_BUFS: [[u8; REQ_BUF_SIZE]; WEB_TASK_POOL_SIZE] =
    [[0; REQ_BUF_SIZE]; WEB_TASK_POOL_SIZE];

// If PSRAM is unavailable at runtime for a worker, fall back to SRAM per-worker.
#[cfg(feature = "psram")]
static mut FALLBACK_RX_BUFS: [[u8; FALLBACK_RX_BUF_SIZE]; WEB_TASK_POOL_SIZE] =
    [[0; FALLBACK_RX_BUF_SIZE]; WEB_TASK_POOL_SIZE];
#[cfg(feature = "psram")]
static mut FALLBACK_TX_BUFS: [[u8; FALLBACK_TX_BUF_SIZE]; WEB_TASK_POOL_SIZE] =
    [[0; FALLBACK_TX_BUF_SIZE]; WEB_TASK_POOL_SIZE];
#[cfg(feature = "psram")]
static mut FALLBACK_HTTP_REQ_BUFS: [[u8; REQ_BUF_SIZE]; WEB_TASK_POOL_SIZE] =
    [[0; REQ_BUF_SIZE]; WEB_TASK_POOL_SIZE];

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

    // Keep connections alive so WS upgrade stays open.
    let cfg = picoserve::Config::new(picoserve::Timeouts {
        start_read_request: TimerDuration::from_secs(5),
        persistent_start_read_request: TimerDuration::from_secs(3),
        read_request: TimerDuration::from_secs(2),
        write: TimerDuration::from_secs(3),
    })
    .keep_connection_alive();

    // --- Buffer selection per worker ---
    #[cfg(feature = "psram")]
    let (rx_buf, tx_buf, http_buf): (&mut [u8], &mut [u8], &mut [u8]) =
        match crate::psram_pool::http_buffers(id) {
            Some((rx, tx)) => {
                let http: &mut [u8] = unsafe { &mut FALLBACK_HTTP_REQ_BUFS[id] };
                (rx, tx, http)
            }
            None => {
                log::warn!("http[{id}]: PSRAM buffers unavailable; falling back to SRAM");
                let rx: &mut [u8] = unsafe { &mut FALLBACK_RX_BUFS[id] };
                let tx: &mut [u8] = unsafe { &mut FALLBACK_TX_BUFS[id] };
                let http: &mut [u8] = unsafe { &mut FALLBACK_HTTP_REQ_BUFS[id] };
                (rx, tx, http)
            }
        };

    #[cfg(not(feature = "psram"))]
    let (rx_buf, tx_buf, http_buf): (&mut [u8], &mut [u8], &mut [u8]) =
        unsafe { (&mut RX_BUFS[id], &mut TX_BUFS[id], &mut HTTP_REQ_BUFS[id]) };

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
