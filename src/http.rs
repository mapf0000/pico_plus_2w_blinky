use core::{fmt::Write as _, str};

use embassy_net::{self as net, tcp::TcpSocket};
use embassy_time::Duration;
use heapless::String;

use crate::hid::{HID_CHAN, HidCommand, USB_READY};
use crate::{USB_ENABLED, USB_START};
use crate::host::{self, HostOs};
use crate::scripts;
use crate::config;

const SERVER_PORT: u16 = 80;
// Centralized HTTP sizes and limits for maintainability
#[cfg(feature = "psram")]
const RX_BUF_SIZE: usize = crate::psram_pool::HTTP_RX_SIZE;
#[cfg(feature = "psram")]
const TX_BUF_SIZE: usize = crate::psram_pool::HTTP_TX_SIZE;
#[cfg(not(feature = "psram"))]
const RX_BUF_SIZE: usize = 1024;
#[cfg(not(feature = "psram"))]
const TX_BUF_SIZE: usize = 1024;
const READ_TIMEOUT_SECS: u64 = 5;
const MAX_BODY_BYTES: usize = 512;
// Request parse buffer (headers + small body staging); keep modest to avoid stack bloat.
const REQ_BUF_SIZE: usize = 1024;

#[embassy_executor::task]
pub async fn server_task(stack: &'static net::Stack<'static>) {
    log::info!("http: listening on port {}", SERVER_PORT);
    #[cfg(feature = "psram")]
    let mut use_psram = false;
    #[cfg(feature = "psram")]
    let (mut rx_ps, mut tx_ps) = match crate::psram_pool::http_buffers() {
        Some((a, b)) => { use_psram = true; (a, b) }
        None => {
            log::warn!("http: PSRAM buffers unavailable; falling back to SRAM");
            // Dummy slices; will not be used when use_psram=false
            (&mut [0u8; 0][..], &mut [0u8; 0][..])
        }
    };

    loop {
        #[cfg(feature = "psram")]
        if use_psram {
            let mut socket = TcpSocket::new(*stack, &mut rx_ps, &mut tx_ps);
            socket.set_timeout(Some(Duration::from_secs(READ_TIMEOUT_SECS)));
            if let Err(e) = socket.accept(SERVER_PORT).await {
                log::warn!("http: accept error: {:?}", e);
                continue;
            }
            log::info!("http: accepted connection");
            if let Err(e) = handle_connection(&mut socket).await {
                log::debug!("http: handle error: {:?}", e);
            }
            continue;
        }

        let mut rx_buf = [0u8; RX_BUF_SIZE];
        let mut tx_buf = [0u8; TX_BUF_SIZE];
        let mut socket = TcpSocket::new(*stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(Duration::from_secs(READ_TIMEOUT_SECS)));
        if let Err(e) = socket.accept(SERVER_PORT).await {
            log::warn!("http: accept error: {:?}", e);
            continue;
        }
        log::info!("http: accepted connection");
        if let Err(e) = handle_connection(&mut socket).await {
            log::debug!("http: handle error: {:?}", e);
        }
    }
}

async fn handle_connection(socket: &mut TcpSocket<'_>) -> Result<(), net::tcp::Error> {
    // Read until we have headers ("\r\n\r\n").
    let mut buf = [0u8; REQ_BUF_SIZE];
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
    // Parse the request line; if invalid, respond 400 rather than defaulting.
    let route = {
        let req = &buf[..n];
        match parse_method_target(req) {
            Some((method, target)) => parse_route(method, target),
            None => {
                respond_text(socket, 400, "bad request\n").await?;
                return Ok(());
            }
        }
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
                } else if content_length > MAX_BODY_BYTES {
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
            // Stream JSON to avoid large on-stack buffers and ensure correctness.
            respond_scripts_list(socket).await?
        }
        // Removed: Route::KbScriptRunBuiltin
        
        Route::ConfigGet => {
            let cfg = config::get().await;
            let man = escape_json_str(cfg.usb_manufacturer.as_str());
            let prod = escape_json_str(cfg.usb_product.as_str());
            let mut body: String<256> = String::new();
            let _ = write!(
                &mut body,
                "{{\"usb_manufacturer\":\"{}\",\"usb_product\":\"{}\"}}\n",
                man.as_str(), prod.as_str()
            );
            respond_bytes(socket, 200, "application/json", body.as_bytes()).await?
        }

        Route::ConfigPost => {
            if USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst) {
                respond_text(socket, 409, "USB already enabled\n").await?
            } else {
                // Re-parse request line to get target with query
                let (method, target) = match parse_method_target(&buf[..n]) {
                    Some(v) => v,
                    None => { respond_text(socket, 400, "bad request\n").await?; return Ok(()); }
                };
                let (_, query) = split_target(target);

                let man_raw = query_param(query, "manufacturer");
                let prod_raw = query_param(query, "product");

                let mut man_dec: Option<heapless::String<{ config::MANUFACTURER_MAX }>> = None;
                let mut prod_dec: Option<heapless::String<{ config::PRODUCT_MAX }>> = None;
                if let Some(m) = man_raw {
                    if let Some(s) = percent_decode_str::<{ config::MANUFACTURER_MAX }>(m) { man_dec = Some(s); }
                    else { respond_text(socket, 400, "bad manufacturer\n").await?; return Ok(()); }
                }
                if let Some(p) = prod_raw {
                    if let Some(s) = percent_decode_str::<{ config::PRODUCT_MAX }>(p) { prod_dec = Some(s); }
                    else { respond_text(socket, 400, "bad product\n").await?; return Ok(()); }
                }

                if let Err(e) = config::set_partial(man_dec.as_deref(), prod_dec.as_deref()).await {
                    match e {
                        config::SetError::TooLongManufacturer | config::SetError::TooLongProduct => {
                            respond_text(socket, 413, "value too long\n").await?
                        }
                        config::SetError::InvalidChars => {
                            respond_text(socket, 400, "invalid characters\n").await?
                        }
                    }
                } else {
                    let _ = config::save().await; // Best-effort
                    respond_text(socket, 200, "ok\n").await?
                }
            }
        }

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
    ConfigGet,
    ConfigPost,
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
        ("GET", "/config") => Route::ConfigGet,
        ("POST", "/config") => Route::ConfigPost,
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
        Route::ConfigGet => "/config",
        Route::ConfigPost => "/config",
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

fn query_param<'a>(query: Option<&'a str>, key: &str) -> Option<&'a str> {
    let q = query?;
    for pair in q.split('&') {
        if let Some(eq) = pair.find('=') {
            let (k, v) = (&pair[..eq], &pair[eq + 1..]);
            if k == key { return Some(v); }
        }
    }
    None
}

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

// Compute length of a JSON-escaped string without allocating.
fn json_escaped_len(s: &str) -> usize {
    let mut n = 0usize;
    for b in s.bytes() {
        n += match b {
            b'"' | b'\\' | b'\n' | b'\r' | b'\t' => 2,
            _ => 1,
        };
    }
    n
}

// Write a JSON-escaped string to socket (no surrounding quotes).
async fn write_json_escaped(socket: &mut TcpSocket<'_>, s: &str) -> Result<(), net::tcp::Error> {
    for b in s.bytes() {
        match b {
            b'"' => write_all(socket, b"\\\"").await?,
            b'\\' => write_all(socket, b"\\\\").await?,
            b'\n' => write_all(socket, b"\\n").await?,
            b'\r' => write_all(socket, b"\\r").await?,
            b'\t' => write_all(socket, b"\\t").await?,
            _ => {
                let ch = [b];
                write_all(socket, &ch).await?;
            }
        }
    }
    Ok(())
}

// Stream the scripts list JSON with precise Content-Length and minimal stack.
async fn respond_scripts_list(socket: &mut TcpSocket<'_>) -> Result<(), net::tcp::Error> {
    // Write headers without Content-Length; body is delimited by connection close.
    let mut headers: String<128> = String::new();
    let _ = write!(
        &mut headers,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nConnection: close\r\n\r\n",
        200u16,
        status_text(200),
        "application/json",
    );
    write_all(socket, headers.as_bytes()).await?;

    // Write body streaming
    write_all(socket, b"[").await?;
    for (i, s) in scripts::SCRIPTS.iter().enumerate() {
        if i != 0 { write_all(socket, b",").await?; }
        // {"id":"
        write_all(socket, b"{\"id\":\"").await?;
        write_json_escaped(socket, s.id).await?;
        // ","name":"
        write_all(socket, b"\",\"name\":\"").await?;
        write_json_escaped(socket, s.name).await?;
        // ","description":"
        write_all(socket, b"\",\"description\":\"").await?;
        write_json_escaped(socket, s.description).await?;
        write_all(socket, b"\"").await?;
        if let Some(pre) = s.dsl_preview {
            write_all(socket, b",\"dsl\":\"").await?;
            write_json_escaped(socket, pre).await?;
            write_all(socket, b"\"").await?;
        }
        write_all(socket, b"}").await?;
    }
    write_all(socket, b"]\n").await?;

    // Close and drain like respond_bytes
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

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn percent_decode_str<const N: usize>(s: &str) -> Option<heapless::String<N>> {
    let bytes = s.as_bytes();
    let mut out: heapless::String<N> = heapless::String::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = hex_val(bytes[i + 1])?;
                let lo = hex_val(bytes[i + 2])?;
                let ch = (hi << 4) | lo;
                if out.push(ch as char).is_err() { return None; }
                i += 3;
            }
            b'+' => {
                if out.push(' ').is_err() { return None; }
                i += 1;
            }
            c => {
                if out.push(c as char).is_err() { return None; }
                i += 1;
            }
        }
    }
    Some(out)
}
