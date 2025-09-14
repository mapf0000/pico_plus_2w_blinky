use core::{fmt::Write as _, str};

use embassy_net::{self as net, tcp::TcpSocket};
use embassy_time::Duration;
use heapless::String;

use crate::hid::{HID_CHAN, HidCommand, USB_READY};
use crate::{USB_ENABLED, USB_START};
use crate::host::{self, HostOs};
use crate::scripts;

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
        Route::UsbRegister { run_assistant, host_os } => {
            if USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst) {
                respond_text(socket, 409, "USB already enabled\n").await?
            } else {
                // Apply host OS from query param only.
                if let Some(os) = host_os {
                    host::set_host_os(os);
                }

                USB_START.signal(run_assistant);
                if run_assistant {
                    respond_text(socket, 200, "USB enabling (assistant)\n").await?
                } else {
                    respond_text(socket, 200, "USB enabling (no assistant)\n").await?
                }
            }
        }
        Route::KbScript => {
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
                } else if content_length > 512 {
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
                        // Validate ASCII-ish payload (tabs/newlines allowed)
                        if !body.iter().all(|b| matches!(b, 9 | 10 | 13 | 32..=126)) {
                            respond_text(socket, 400, "invalid characters\n").await?
                        } else {
                            let mut s: heapless::String<512> = heapless::String::new();
                            for &ch in body {
                                if s.push(ch as char).is_err() {
                                    respond_text(socket, 413, "payload too large\n").await?;
                                    return Ok(());
                                }
                            }
                            match HID_CHAN.try_send(HidCommand::RunDsl { dsl: s }) {
                                Ok(()) => respond_text(socket, 202, "queued\n").await?,
                                Err(_) => respond_text(socket, 409, "busy\n").await?,
                            }
                        }
                    }
                }
            }
        }
        Route::KbScriptsList => {
            // Build a small JSON array of script metadata.
            let mut body: String<768> = String::new();
            let _ = write!(&mut body, "[");
            for (i, s) in scripts::SCRIPTS.iter().enumerate() {
                if i != 0 { let _ = write!(&mut body, ","); }
                let _ = write!(
                    &mut body,
                    "{{\"id\":\"{}\",\"name\":\"{}\",\"description\":\"{}\"",
                    s.id, s.name, s.description
                );
                if let Some(pre) = s.dsl_preview {
                    // Escape basic characters for JSON
                    let esc = escape_json_str(pre);
                    let _ = write!(&mut body, ",\"dsl\":\"{}\"", esc.as_str());
                }
                let _ = write!(&mut body, "}}");
            }
            let _ = write!(&mut body, "]\n");
            respond_bytes(socket, 200, "application/json", body.as_bytes()).await?
        }
        // Removed: Route::KbScriptRunBuiltin
        
        Route::Status => {
            let enabled = USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst);
            let ready = USB_READY.load(core::sync::atomic::Ordering::SeqCst);
            let mut body: String<96> = String::new();
            let _ = write!(
                &mut body,
                "{{\"usb_enabled\":{},\"usb_ready\":{},\"host_os\":\"{}\"}}\n",
                enabled, ready, host::host_os_str()
            );
            respond_bytes(socket, 200, "application/json", body.as_bytes()).await?
        }
        Route::Root => {
            // Serve a tiny UI so users can interact from a browser.
            // Kept inline via include_str! to avoid heap usage at runtime.
            static INDEX_HTML: &str = include_str!("../assets/index.html");
            respond_bytes(socket, 200, "text/html; charset=utf-8", INDEX_HTML.as_bytes()).await?
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
    UsbRegister { run_assistant: bool, host_os: Option<HostOs> },
    KbScript,
    KbScriptsList,
    Status,
    Root,
    NotFound,
}

fn parse_route(method: &str, target: &str) -> Route {
    let (path, query) = split_target(target);
    match (method, path) {
        ("POST", "/usb/register") => {
            let run_assistant = query_flag(query, "assistant").unwrap_or(true);
            let host_os = query_os(query);
            Route::UsbRegister { run_assistant, host_os }
        }
        ("POST", "/kb/script") => Route::KbScript,
        ("GET", "/kb/scripts") => Route::KbScriptsList,
        ("GET", "/status") => Route::Status,
        ("GET", "/") => Route::Root,
        _ => Route::NotFound,
    }
}

fn route_name(r: &Route) -> &'static str {
    match r {
        Route::UsbRegister { .. } => "/usb/register",
        Route::KbScript => "/kb/script",
        Route::KbScriptsList => "/kb/scripts",
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

// query_u64 removed; no numeric query parameters remain

fn query_os(query: Option<&str>) -> Option<HostOs> {
    let q = query?;
    for pair in q.split('&') {
        if let Some(eq) = pair.find('=') {
            let (k, v) = (&pair[..eq], &pair[eq + 1..]);
            if k == "os" {
                if v.eq_ignore_ascii_case("mac") {
                    return Some(HostOs::Mac);
                } else if v.eq_ignore_ascii_case("windows") {
                    return Some(HostOs::Windows);
                }
            }
        }
    }
    None
}

// query_str removed (no longer needed)

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

fn escape_json_str(s: &str) -> heapless::String<512> {
    let mut out: heapless::String<512> = heapless::String::new();
    for b in s.bytes() {
        match b {
            b'"' => { let _ = out.push_str("\\\""); }
            b'\\' => { let _ = out.push_str("\\\\"); }
            b'\n' => { let _ = out.push_str("\\n"); }
            b'\r' => { let _ = out.push_str("\\r"); }
            b'\t' => { let _ = out.push_str("\\t"); }
            _ => { let _ = out.push(b as char); }
        }
    }
    out
}
