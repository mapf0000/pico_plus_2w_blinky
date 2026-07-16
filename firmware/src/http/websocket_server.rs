use embassy_net as net;
use embassy_time::Duration;

const WEBSOCKET_PORT: u16 = transfer_protocol::WEBSOCKET_PORT;

pub fn spawn_websocket_server(
    spawner: &embassy_executor::Spawner,
    stack: net::Stack<'static>,
) -> bool {
    let spawned = crate::log_spawn(
        spawner,
        "http::websocket_server_task",
        websocket_server_task(stack),
    );
    if spawned {
        log::info!("websocket: singleton server listening on port {WEBSOCKET_PORT}");
    }
    spawned
}

#[embassy_executor::task]
async fn websocket_server_task(stack: net::Stack<'static>) -> ! {
    let app = crate::http::routes::websocket_router!();
    let config = picoserve::Config::new(picoserve::Timeouts {
        start_read_request: Duration::from_secs(5),
        persistent_start_read_request: Duration::from_secs(3),
        read_request: Duration::from_secs(2),
        write: Duration::from_secs(10),
    })
    .keep_connection_alive();

    let (sram_rx, sram_tx, request_buffer) =
        super::server::take_sram_buffers(super::server::WEBSOCKET_BUFFER_SLOT)
            .expect("the singleton WebSocket server has one statically assigned buffer slot");

    #[cfg(feature = "psram")]
    let (rx_buffer, tx_buffer) = match crate::psram_pool::take_websocket_buffers() {
        Some(buffers) => buffers,
        None => {
            log::warn!("websocket: PSRAM TCP buffers unavailable; using SRAM");
            (sram_rx, sram_tx)
        }
    };

    #[cfg(not(feature = "psram"))]
    let (rx_buffer, tx_buffer) = (sram_rx, sram_tx);

    log::info!(
        "websocket: buffers ready (rx={}, tx={}, req={})",
        rx_buffer.len(),
        tx_buffer.len(),
        request_buffer.len()
    );

    picoserve::Server::new(&app, &config, request_buffer)
        .listen_and_serve("websocket", stack, WEBSOCKET_PORT, rx_buffer, tx_buffer)
        .await
        .into_never()
}
