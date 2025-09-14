use core::{fmt::Write as _, str};

use embassy_net::{self as net, tcp::TcpSocket};
use embassy_time::Duration;
use heapless::String;

use crate::hid::{HID_CHAN, HidCommand, USB_READY};
use crate::{USB_ENABLED, USB_START};

const SERVER_PORT: u16 = 80;

#[embassy_executor::task]
pub async fn server_task(stack: &'static net::Stack<'static>) {
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

async fn handle_connection(socket: &mut TcpSocket<'_>) -> Result<(), net::tcp::Error> {
    // Read until we have headers ("\r\n\r\n").
    let mut buf = [0u8; 1024];
    let mut n = 0;
    let mut header_end: Option<usize> = None;
    loop {
        let m = socket.read(&mut buf[n..]).await?;
        if m == 0 {
            break;
        }
        n += m;
        if let Some(idx) = find_dbl_crlf(&buf[..n]) {
            header_end = Some(idx);
            break;
        }
        if n == buf.len() {
            // Too large headers
            respond_text(socket, 400, "bad request\n").await?;
            return Ok(());
        }
    }
    // Parse route without holding onto header borrows.
    let route = {
        let req = &buf[..n];
        let (method, target) = parse_method_target(req).unwrap_or(("GET", "/"));
        parse_route(method, target)
    };

    log::info!("http: route {:?} ({} bytes)", route_name(&route), n);

    match route {
        Route::UsbRegister { run_assistant } => {
            if USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst) {
                respond_text(socket, 409, "USB already enabled\n").await?
            } else {
                USB_START.signal(run_assistant);
                if run_assistant {
                    respond_text(socket, 200, "USB enabling (assistant)\n").await?
                } else {
                    respond_text(socket, 200, "USB enabling (no assistant)\n").await?
                }
            }
        }
        Route::KbType { delay_ms } => {
            // Verify USB ready
            if !USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst)
                || !USB_READY.load(core::sync::atomic::Ordering::SeqCst)
            {
                respond_text(socket, 409, "USB not ready\n").await?
            } else {
                // Content-Length and body
                let header_end = match header_end {
                    Some(v) => v,
                    None => {
                        respond_text(socket, 400, "bad request\n").await?;
                        return Ok(());
                    }
                };
                let headers = &buf[..header_end];
                let content_length = parse_content_length(headers).unwrap_or(0);
                if content_length == 0 {
                    respond_text(socket, 400, "empty body\n").await?
                } else if content_length > 256 {
                    respond_text(socket, 413, "payload too large\n").await?
                } else {
                    let body_start = header_end + 4;
                    let mut have = n.saturating_sub(body_start);
                    // Read remaining body if needed
                    while have < content_length && n < buf.len() {
                        let m = socket.read(&mut buf[n..]).await?;
                        if m == 0 {
                            break;
                        }
                        n += m;
                        have = n - body_start;
                    }
                    if have < content_length {
                        respond_text(socket, 400, "incomplete body\n").await?
                    } else {
                        let body = &buf[body_start..body_start + content_length];
                        let text = match core::str::from_utf8(body) {
                            Ok(s) => s,
                            Err(_) => {
                                respond_text(socket, 400, "invalid utf-8\n").await?;
                                return Ok(());
                            }
                        };
                        // Validate characters
                        if !text
                            .chars()
                            .all(|c| crate::keyboard::char_to_key(c).is_some())
                        {
                            respond_text(socket, 400, "unsupported character\n").await?;
                        } else {
                            let mut s: heapless::String<256> = heapless::String::new();
                            if s.push_str(text).is_err() {
                                respond_text(socket, 413, "payload too large\n").await?
                            } else {
                                match HID_CHAN.try_send(HidCommand::Type { text: s, delay_ms }) {
                                    Ok(()) => respond_text(socket, 202, "queued\n").await?,
                                    Err(_) => respond_text(socket, 409, "busy\n").await?,
                                }
                            }
                        }
                    }
                }
            }
        }
        Route::AutomationOpenMacTerminal => {
            if !USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst)
                || !USB_READY.load(core::sync::atomic::Ordering::SeqCst)
            {
                respond_text(socket, 409, "USB not ready\n").await?
            } else {
                match HID_CHAN.try_send(HidCommand::OpenMacTerminal) {
                    Ok(()) => respond_text(socket, 202, "queued\n").await?,
                    Err(_) => respond_text(socket, 409, "busy\n").await?,
                }
            }
        }
        Route::AutomationMacAssistant => {
            if !USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst)
                || !USB_READY.load(core::sync::atomic::Ordering::SeqCst)
            {
                respond_text(socket, 409, "USB not ready\n").await?
            } else {
                match HID_CHAN.try_send(HidCommand::MacAssistant) {
                    Ok(()) => respond_text(socket, 202, "queued\n").await?,
                    Err(_) => respond_text(socket, 409, "busy\n").await?,
                }
            }
        }
        Route::Status => {
            let enabled = USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst);
            let ready = USB_READY.load(core::sync::atomic::Ordering::SeqCst);
            let mut body: String<96> = String::new();
            let _ = write!(
                &mut body,
                "{{\"usb_enabled\":{},\"usb_ready\":{}}}\n",
                enabled, ready
            );
            respond_bytes(socket, 200, "application/json", body.as_bytes()).await?
        }
        Route::Root => {
            let body = b"OK\n";
            respond_bytes(socket, 200, "text/plain", body).await?
        }
        Route::NotFound => respond_text(socket, 404, "not found\n").await?,
    }

    Ok(())
}

fn parse_method_target(req: &[u8]) -> Option<(&str, &str)> {
    // Very small parser: "METHOD /path HTTP/1.1\r\n..."
    let s = str::from_utf8(req).ok()?;
    let mut lines = s.split('\n');
    let line = lines.next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    Some((method, target))
}

fn split_target(target: &str) -> (&str, Option<&str>) {
    if let Some(i) = target.find('?') {
        (&target[..i], Some(&target[i + 1..]))
    } else {
        (target, None)
    }
}

#[derive(Debug)]
enum Route {
    UsbRegister { run_assistant: bool },
    KbType { delay_ms: u64 },
    AutomationOpenMacTerminal,
    AutomationMacAssistant,
    Status,
    Root,
    NotFound,
}

fn parse_route(method: &str, target: &str) -> Route {
    let (path, query) = split_target(target);
    match (method, path) {
        ("POST", "/usb/register") => {
            let run_assistant = query_flag(query, "assistant").unwrap_or(true);
            Route::UsbRegister { run_assistant }
        }
        ("POST", "/kb/type") => {
            let delay_ms = query_u64(query, "delay_ms").unwrap_or(10);
            Route::KbType { delay_ms }
        }
        ("POST", "/automation/open_macos_terminal") => Route::AutomationOpenMacTerminal,
        ("POST", "/automation/mac_assistant") => Route::AutomationMacAssistant,
        ("GET", "/status") => Route::Status,
        ("GET", "/") => Route::Root,
        _ => Route::NotFound,
    }
}

fn route_name(r: &Route) -> &'static str {
    match r {
        Route::UsbRegister { .. } => "/usb/register",
        Route::KbType { .. } => "/kb/type",
        Route::AutomationOpenMacTerminal => "/automation/open_macos_terminal",
        Route::AutomationMacAssistant => "/automation/mac_assistant",
        Route::Status => "/status",
        Route::Root => "/",
        Route::NotFound => "notfound",
    }
}

fn query_flag(query: Option<&str>, key: &str) -> Option<bool> {
    let q = query?;
    for pair in q.split('&') {
        if let Some(eq) = pair.find('=') {
            let (k, v) = (&pair[..eq], &pair[eq + 1..]);
            if k == key {
                if v.eq_ignore_ascii_case("true") || v == "1" {
                    return Some(true);
                }
                if v.eq_ignore_ascii_case("false") || v == "0" {
                    return Some(false);
                }
            }
        } else if pair == key {
            return Some(true);
        }
    }
    None
}

fn query_u64(query: Option<&str>, key: &str) -> Option<u64> {
    let q = query?;
    for pair in q.split('&') {
        if let Some(eq) = pair.find('=') {
            let (k, v) = (&pair[..eq], &pair[eq + 1..]);
            if k == key {
                if let Ok(val) = v.parse::<u64>() {
                    return Some(val);
                }
            }
        }
    }
    None
}

fn find_dbl_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let s = core::str::from_utf8(headers).ok()?;
    for line in s.lines() {
        // Case-insensitive match for content-length
        let (name, val) = match line.split_once(':') {
            Some(v) => v,
            None => continue,
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            if let Ok(n) = val.trim().parse::<usize>() {
                return Some(n);
            }
        }
    }
    None
}

async fn respond_text(
    socket: &mut TcpSocket<'_>,
    code: u16,
    body: &str,
) -> Result<(), net::tcp::Error> {
    respond_bytes(socket, code, "text/plain", body.as_bytes()).await
}

async fn respond_bytes(
    socket: &mut TcpSocket<'_>,
    code: u16,
    content_type: &str,
    body: &[u8],
) -> Result<(), net::tcp::Error> {
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
    // Gracefully close the sending side, then drain until peer closes.
    // Draining avoids lwIP/embassy-net sending RST if unread data remains.
    socket.close();
    let mut drain = [0u8; 128];
    loop {
        match socket.read(&mut drain).await {
            Ok(0) => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    Ok(())
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        413 => "Payload Too Large",
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
