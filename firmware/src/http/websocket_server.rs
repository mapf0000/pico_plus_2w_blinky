use embassy_net as net;
use embassy_time::Duration;

const WEBSOCKET_PORT: u16 = transfer_protocol::WEBSOCKET_PORT;
pub(super) const WEBSOCKET_TASK_POOL_SIZE: usize = 2;

pub fn spawn_websocket_server(
    spawner: &embassy_executor::Spawner,
    stack: net::Stack<'static>,
) -> bool {
    let mut spawned_all = true;
    for id in 0..WEBSOCKET_TASK_POOL_SIZE {
        let spawned = crate::log_spawn(
            spawner,
            "http::websocket_server_task",
            websocket_server_task(id, stack),
        );
        spawned_all &= spawned;
        if spawned {
            log::info!("websocket[{id}]: acceptor listening on port {WEBSOCKET_PORT}");
        }
    }
    spawned_all
}

#[embassy_executor::task(pool_size = WEBSOCKET_TASK_POOL_SIZE)]
async fn websocket_server_task(id: usize, stack: net::Stack<'static>) -> ! {
    let app = crate::http::routes::websocket_router!();
    let config = picoserve::Config::new(picoserve::Timeouts {
        start_read_request: Duration::from_secs(5),
        persistent_start_read_request: Duration::from_secs(3),
        read_request: Duration::from_secs(2),
        write: Duration::from_secs(10),
    })
    .close_connection_after_response();

    let (sram_rx, sram_tx, request_buffer) =
        super::server::take_sram_buffers(super::server::WEBSOCKET_BUFFER_SLOT_START + id)
            .expect("each WebSocket acceptor has one statically assigned buffer slot");

    #[cfg(feature = "psram")]
    let (rx_buffer, tx_buffer) = match crate::psram_pool::take_websocket_buffers(id) {
        Some(buffers) => buffers,
        None => {
            log::warn!("websocket[{id}]: PSRAM TCP buffers unavailable; using SRAM");
            (sram_rx, sram_tx)
        }
    };

    #[cfg(not(feature = "psram"))]
    let (rx_buffer, tx_buffer) = (sram_rx, sram_tx);

    log::info!(
        "websocket[{id}]: buffers ready (rx={}, tx={}, req={})",
        rx_buffer.len(),
        tx_buffer.len(),
        request_buffer.len()
    );

    picoserve::Server::new(&app, &config, request_buffer)
        .listen_and_serve(id, stack, WEBSOCKET_PORT, rx_buffer, tx_buffer)
        .await
        .into_never()
}

#[cfg(feature = "psram")]
const _: () = assert!(WEBSOCKET_TASK_POOL_SIZE == crate::psram_pool::WEBSOCKET_BUFFER_COUNT);
