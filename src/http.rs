use core::{fmt::Write as _, str};

use embassy_net::{self as net, tcp::TcpSocket};
use embassy_time::Duration;
use heapless::String;

use crate::temp::{self, Shared};

const SERVER_PORT: u16 = 80;

#[embassy_executor::task]
pub async fn server_task(
    stack: &'static net::Stack<'static>,
    shared: &'static Shared,
){
    log::info!("http: listening on port {}", SERVER_PORT);
    loop {
        let mut rx_buf = [0u8; 1024];
        let mut tx_buf = [0u8; 1024];
        let mut socket = TcpSocket::new(*stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(Duration::from_secs(5)));

        if let Err(e) = socket.accept(SERVER_PORT).await {
            log::warn!("http: accept error: {:?}", e);
            continue;
        }
        log::info!("http: accepted connection");

        // Process single request per connection; then close.
        if let Err(e) = handle_connection(&mut socket, shared).await {
            log::debug!("http: handle error: {:?}", e);
        }
    }
}

async fn handle_connection(
    socket: &mut TcpSocket<'_>,
    shared: &'static Shared,
) -> Result<(), net::tcp::Error> {
    let mut buf = [0u8; 512];
    let n = socket.read(&mut buf).await?;
    let req = &buf[..n];
    let path = parse_path(req).unwrap_or("/");
    log::info!("http: request path '{}' ({} bytes)", path, n);

    match path {
        "/temp" => {
            log::info!("http: responding 200 JSON");
            respond_json(socket, shared).await?
        }
        "/metrics" => {
            log::info!("http: responding 200 metrics");
            respond_metrics(socket, shared).await?
        }
        _ => {
            log::info!("http: responding 404 for '{}'", path);
            respond_not_found(socket, path).await?
        }
    }

    Ok(())
}

fn parse_path(req: &[u8]) -> Option<&str> {
    // Very small parser: "GET /path HTTP/1.1\r\n..."
    let s = str::from_utf8(req).ok()?;
    let mut lines = s.split('\n');
    let line = lines.next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    if method != "GET" { return None; }
    parts.next()
}

async fn respond_json(socket: &mut TcpSocket<'_>, shared: &Shared) -> Result<(), net::tcp::Error> {
    let reading = shared.get().await;

    let mut body: String<128> = String::new();
    temp::write_json(&mut body, reading);

    let mut headers: String<128> = String::new();
    let _ = write!(
        &mut headers,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    write_all(socket, headers.as_bytes()).await?;
    write_all(socket, body.as_bytes()).await?;
    // Gracefully close so clients don't see a TCP RST
    socket.close();
    // Optionally wait for peer to ack/close; ignore outcome (bounded by timeout)
    let _ = socket.read(&mut [0u8; 1]).await;
    Ok(())
}

async fn respond_metrics(socket: &mut TcpSocket<'_>, shared: &Shared) -> Result<(), net::tcp::Error> {
    let r = shared.get().await;
    let mut body: String<160> = String::new();
    let _ = write!(&mut body, "pico_temperature_celsius {:.2}\n", r.celsius);

    let mut headers: String<128> = String::new();
    let _ = write!(
        &mut headers,
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    write_all(socket, headers.as_bytes()).await?;
    write_all(socket, body.as_bytes()).await?;
    socket.close();
    let _ = socket.read(&mut [0u8; 1]).await;
    Ok(())
}

async fn respond_not_found(socket: &mut TcpSocket<'_>, _path: &str) -> Result<(), net::tcp::Error> {
    let body = b"{\"error\":\"not found\"}";
    let mut headers: String<128> = String::new();
    let _ = write!(
        &mut headers,
        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    write_all(socket, headers.as_bytes()).await?;
    write_all(socket, body).await?;
    socket.close();
    let _ = socket.read(&mut [0u8; 1]).await;
    Ok(())
}

// Ensure we transmit the full buffer before closing.
async fn write_all(socket: &mut TcpSocket<'_>, mut buf: &[u8]) -> Result<(), net::tcp::Error> {
    while !buf.is_empty() {
        let n = socket.write(buf).await?;
        if n == 0 {
            break;
        }
        buf = &buf[n..];
    }
    Ok(())
}
