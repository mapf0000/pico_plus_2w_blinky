use core::{fmt::Write as _, str};

use embassy_net::{self as net, tcp::TcpSocket};
use embassy_time::Duration;
use heapless::String;

use crate::{USB_ENABLED, USB_START};

const SERVER_PORT: u16 = 80;

#[embassy_executor::task]
pub async fn server_task(
    stack: &'static net::Stack<'static>,
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
        if let Err(e) = handle_connection(&mut socket).await {
            log::debug!("http: handle error: {:?}", e);
        }
    }
}

async fn handle_connection(
    socket: &mut TcpSocket<'_>,
) -> Result<(), net::tcp::Error> {
    let mut buf = [0u8; 512];
    let n = socket.read(&mut buf).await?;
    let req = &buf[..n];
    let (method, path) = parse_method_path(req).unwrap_or(("GET", "/"));
    log::info!("http: {} '{}' ({} bytes)", method, path, n);

    match (method, path) {
        ("POST", "/usb/register") => {
            if USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst) {
                respond_text(socket, 409, "USB already enabled\n").await?
            } else {
                USB_START.signal(());
                respond_text(socket, 200, "USB enabling\n").await?
            }
        }
        ("GET", "/") => {
            let body = b"OK\n";
            respond_bytes(socket, 200, "text/plain", body).await?
        }
        _ => {
            respond_text(socket, 404, "not found\n").await?
        }
    }

    Ok(())
}

fn parse_method_path(req: &[u8]) -> Option<(&str, &str)> {
    // Very small parser: "METHOD /path HTTP/1.1\r\n..."
    let s = str::from_utf8(req).ok()?;
    let mut lines = s.split('\n');
    let line = lines.next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let path = parts.next()?;
    Some((method, path))
}

async fn respond_text(socket: &mut TcpSocket<'_>, code: u16, body: &str) -> Result<(), net::tcp::Error> {
    respond_bytes(socket, code, "text/plain", body.as_bytes()).await
}

async fn respond_bytes(socket: &mut TcpSocket<'_>, code: u16, content_type: &str, body: &[u8]) -> Result<(), net::tcp::Error> {
    let mut headers: String<160> = String::new();
    let _ = write!(
        &mut headers,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        code,
        status_text(code),
        content_type,
        body.len()
    );
    write_all(socket, headers.as_bytes()).await?;
    write_all(socket, body).await?;
    socket.close();
    let _ = socket.read(&mut [0u8; 1]).await;
    Ok(())
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        404 => "Not Found",
        409 => "Conflict",
        _ => "OK",
    }
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
