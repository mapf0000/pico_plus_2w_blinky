use embassy_futures::select::{Either as SelectEither, select};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};
use picoserve::futures::Either;
use picoserve::io::embedded_io_async;
use picoserve::response::ws; // for Read/Write trait bounds
use portable_atomic::{AtomicUsize, Ordering};

use crate::host::{self, HostOs};
use crate::http::util::{escape_json_str, percent_decode_str};
use crate::usb::ctrl::{CTRL_CHAN, CtrlCommand, MAX_SECURE_TRANSFER_FRAME, MAX_TRANSFER_PATH_LEN};
use crate::usb::hid::{HID_CHAN, HidCommand, MAX_BYTECODE, USB_READY};
use crate::usb::usb_supervisor;
use heapless::{String, Vec};

pub const TRANSFER_TEXT_MAX: usize = 768;
pub const TRANSFER_BINARY_MAX: usize = 2049;
pub const WS_BINARY_KIND_FILESYSTEM: u8 = 2;
pub const WS_BINARY_KIND_SESSION: u8 = transfer_protocol::WS_BINARY_KIND_SESSION;
const WS_COMMAND_MAX: usize = 2048;
const _: () = assert!(TRANSFER_TEXT_MAX <= TRANSFER_BINARY_MAX);

#[derive(Clone, Copy)]
enum TransferWsEventKind {
    Text,
    Binary,
}

pub struct TransferWsEvent {
    kind: TransferWsEventKind,
    payload: Vec<u8, TRANSFER_BINARY_MAX>,
}

pub static TRANSFER_WS_EVENTS: Channel<ThreadModeRawMutex, TransferWsEvent, 16> = Channel::new();
static ACTIVE_WS_CLIENTS: AtomicUsize = AtomicUsize::new(0);

pub fn has_active_client() -> bool {
    ACTIVE_WS_CLIENTS.load(Ordering::Acquire) > 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferQueueError {
    NoClient,
    Full,
}

pub fn queue_transfer_text(event: String<TRANSFER_TEXT_MAX>) -> Result<(), TransferQueueError> {
    let mut payload = Vec::new();
    payload
        .extend_from_slice(event.as_bytes())
        .expect("text capacity is bounded by the WebSocket payload capacity");
    queue_transfer_event(TransferWsEvent {
        kind: TransferWsEventKind::Text,
        payload,
    })
}

pub async fn send_transfer_text(
    event: String<TRANSFER_TEXT_MAX>,
) -> Result<(), TransferQueueError> {
    let mut payload = Vec::new();
    payload
        .extend_from_slice(event.as_bytes())
        .expect("text capacity is bounded by the WebSocket payload capacity");
    enqueue_transfer_event(TransferWsEvent {
        kind: TransferWsEventKind::Text,
        payload,
    })
    .await
}

pub fn queue_transfer_binary(
    event: Vec<u8, TRANSFER_BINARY_MAX>,
) -> Result<(), TransferQueueError> {
    queue_transfer_event(TransferWsEvent {
        kind: TransferWsEventKind::Binary,
        payload: event,
    })
}

/// Enqueue transfer data without dropping it when USB temporarily outpaces Wi-Fi.
///
/// The USB control task awaits this function before acknowledging the chunk to
/// the host agent, propagating WebSocket backpressure across the whole transfer
/// pipeline.
pub async fn send_transfer_binary(
    event: Vec<u8, TRANSFER_BINARY_MAX>,
) -> Result<(), TransferQueueError> {
    enqueue_transfer_event(TransferWsEvent {
        kind: TransferWsEventKind::Binary,
        payload: event,
    })
    .await
}

async fn enqueue_transfer_event(event: TransferWsEvent) -> Result<(), TransferQueueError> {
    if !has_active_client() {
        return Err(TransferQueueError::NoClient);
    }

    TRANSFER_WS_EVENTS.send(event).await;

    // A disconnect drains the channel to wake blocked senders. Do not report
    // that wake-up as a successfully relayed chunk.
    if !has_active_client() {
        return Err(TransferQueueError::NoClient);
    }

    Ok(())
}

fn queue_transfer_event(event: TransferWsEvent) -> Result<(), TransferQueueError> {
    if !has_active_client() {
        return Err(TransferQueueError::NoClient);
    }

    TRANSFER_WS_EVENTS
        .try_send(event)
        .map_err(|_| TransferQueueError::Full)
}

fn begin_ws_session() {
    if ACTIVE_WS_CLIENTS.fetch_add(1, Ordering::AcqRel) == 0 {
        drain_transfer_events();
    }
}

fn end_ws_session() {
    let previous = ACTIVE_WS_CLIENTS.fetch_sub(1, Ordering::AcqRel);
    if previous <= 1 {
        ACTIVE_WS_CLIENTS.store(0, Ordering::Release);
        drain_transfer_events();
    }
}

fn drain_transfer_events() {
    while TRANSFER_WS_EVENTS.try_receive().is_ok() {}
}

pub(crate) async fn ws_handler(
    upgrade: ws::WebSocketUpgrade,
) -> impl picoserve::response::IntoResponse {
    upgrade.on_upgrade(HelloWs)
}

struct HelloWs;

impl ws::WebSocketCallback for HelloWs {
    async fn run<R: embedded_io_async::Read, W: embedded_io_async::Write<Error = R::Error>>(
        self,
        mut rx: ws::SocketRx<R>,
        mut tx: ws::SocketTx<W>,
    ) -> Result<(), W::Error> {
        // The first application message is a versioned capability snapshot.
        let hello = crate::capabilities::hello_json();
        tx.send_text(hello.as_str()).await?;
        begin_ws_session();
        let _ = CTRL_CHAN.try_send(CtrlCommand::RequestStatus);

        let mut buf = [0u8; WS_COMMAND_MAX];
        loop {
            match select(
                TRANSFER_WS_EVENTS.receive(),
                rx.next_message(&mut buf, core::future::pending::<()>()),
            )
            .await
            {
                SelectEither::First(event) => {
                    if send_transfer_event(event, &mut tx).await.is_err() {
                        break;
                    }
                }
                SelectEither::Second(result) => match result {
                    Ok(Either::First(Ok(ws::Message::Text(s)))) => {
                        let cmd = s.trim();
                        if cmd.is_empty() {
                            continue;
                        }
                        if let Some((request_id, rpc_cmd)) = parse_rpc_command(cmd) {
                            let response = handle_command(rpc_cmd).await;
                            let mut envelope: String<TRANSFER_TEXT_MAX> = String::new();
                            let _ = core::fmt::write(
                                &mut envelope,
                                format_args!(
                                    "{{\"event_type\":\"command/response\",\"version\":{},\"request_id\":{},\"payload\":{}}}",
                                    crate::capabilities::WEBSOCKET_PROTOCOL_VERSION,
                                    request_id,
                                    response.as_str()
                                ),
                            );
                            if tx.send_text(envelope.as_str()).await.is_err() {
                                break;
                            }
                            continue;
                        }

                        let response = handle_command(cmd).await;
                        if tx.send_text(response.as_str()).await.is_err() {
                            break;
                        }
                    }
                    Ok(Either::First(Ok(ws::Message::Binary(binary)))) => {
                        let Some((&kind, body)) = binary.split_first() else {
                            continue;
                        };
                        if kind != WS_BINARY_KIND_SESSION || body.len() > MAX_SECURE_TRANSFER_FRAME
                        {
                            continue;
                        }
                        let mut payload: Vec<u8, MAX_SECURE_TRANSFER_FRAME> = Vec::new();
                        if payload.extend_from_slice(body).is_ok() {
                            let _ = CTRL_CHAN.try_send(CtrlCommand::SecureTransfer { payload });
                        }
                    }
                    Ok(Either::First(Ok(ws::Message::Ping(p)))) => {
                        if tx.send_pong(p).await.is_err() {
                            break;
                        }
                    }
                    Ok(Either::First(Ok(ws::Message::Pong(_)))) => {}
                    Ok(Either::First(Ok(ws::Message::Close(_)))) => break,
                    Ok(Either::First(Err(_))) => break,
                    Ok(Either::Second(_)) => break,
                    Err(_) => break,
                },
            }
        }

        end_ws_session();
        tx.close(None).await
    }
}

async fn send_transfer_event<W: embedded_io_async::Write>(
    event: TransferWsEvent,
    tx: &mut ws::SocketTx<W>,
) -> Result<(), W::Error> {
    match event.kind {
        TransferWsEventKind::Text => {
            let text = core::str::from_utf8(event.payload.as_slice())
                .expect("text WebSocket events originate from UTF-8 strings");
            tx.send_text(text).await
        }
        TransferWsEventKind::Binary => tx.send_binary(event.payload.as_slice()).await,
    }
}

// ---- WS command handling ----

fn parse_rpc_command(cmd: &str) -> Option<(u64, &str)> {
    let rest = cmd.strip_prefix("RPC ")?;
    let (request_id, rpc_cmd) = rest.split_once(' ')?;
    let request_id = request_id.parse::<u64>().ok()?;
    Some((request_id, rpc_cmd.trim()))
}

/// Handle a single text command and return a JSON response body.
async fn handle_command(cmd: &str) -> String<TRANSFER_TEXT_MAX> {
    let mut response: String<TRANSFER_TEXT_MAX> = String::new();

    if cmd.eq_ignore_ascii_case("HELLO") {
        return crate::capabilities::hello_json();
    }

    if cmd.eq_ignore_ascii_case("STATUS") {
        let enabled = usb_supervisor::USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst);
        let ready = USB_READY.load(core::sync::atomic::Ordering::SeqCst);
        let mut body: String<96> = String::new();
        let _ = core::fmt::write(
            &mut body,
            format_args!(
                "{{\"usb_enabled\":{},\"usb_ready\":{},\"host_os\":\"{}\"}}",
                enabled,
                ready,
                host::host_os_str()
            ),
        );
        let _ = response.push_str(body.as_str());
        return response;
    }

    if cmd.eq_ignore_ascii_case("CONFIG_GET") {
        let cfg = crate::device_config::get().await;
        let man = escape_json_str(cfg.usb_manufacturer.as_str());
        let prod = escape_json_str(cfg.usb_product.as_str());
        let mut body: String<256> = String::new();
        let _ = core::fmt::write(
            &mut body,
            format_args!(
                "{{\"usb_manufacturer\":\"{}\",\"usb_product\":\"{}\"}}",
                man.as_str(),
                prod.as_str()
            ),
        );
        let _ = response.push_str(body.as_str());
        return response;
    }

    if let Some(rest) = cmd.strip_prefix("CONFIG_SET ") {
        // Parse query string: manufacturer=..&product=..
        let mut man_dec: Option<heapless::String<{ crate::device_config::MANUFACTURER_MAX }>> =
            None;
        let mut prod_dec: Option<heapless::String<{ crate::device_config::PRODUCT_MAX }>> = None;
        for pair in rest.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == "manufacturer" {
                    man_dec = percent_decode_str::<{ crate::device_config::MANUFACTURER_MAX }>(v);
                } else if k == "product" {
                    prod_dec = percent_decode_str::<{ crate::device_config::PRODUCT_MAX }>(v);
                }
            }
        }
        if usb_supervisor::USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst) {
            let _ = response.push_str("{\"error\":\"USB already enabled\"}");
            return response;
        }
        match crate::device_config::set_partial(man_dec.as_deref(), prod_dec.as_deref()).await {
            Ok(()) => match crate::device_config::save().await {
                Ok(()) => {
                    let _ = response.push_str("{\"ok\":true}");
                    response
                }
                Err(_) => {
                    let _ = response.push_str("{\"error\":\"persist failed\"}");
                    response
                }
            },
            Err(crate::device_config::SetError::TooLongManufacturer) => {
                let _ = response.push_str("{\"error\":\"bad manufacturer\"}");
                response
            }
            Err(crate::device_config::SetError::TooLongProduct) => {
                let _ = response.push_str("{\"error\":\"bad product\"}");
                response
            }
            Err(crate::device_config::SetError::InvalidChars) => {
                let _ = response.push_str("{\"error\":\"invalid characters\"}");
                response
            }
        }
    } else if cmd.eq_ignore_ascii_case("USB_REGISTER") || cmd.starts_with("USB_REGISTER ") {
        // optional: USB_REGISTER assistant=1&os=mac
        let mut run_assistant = false;
        if let Some(q) = cmd.strip_prefix("USB_REGISTER ") {
            for pair in q.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    if k == "os" {
                        match v {
                            "mac" => host::set_host_os(HostOs::Mac),
                            "windows" => host::set_host_os(HostOs::Windows),
                            _ => host::set_host_os(HostOs::Unknown),
                        }
                    } else if k == "assistant" {
                        run_assistant = v != "0";
                    }
                }
            }
        }
        match usb_supervisor::start(run_assistant).await {
            Ok(()) => {
                let _ = response.push_str("{\"ok\":true}");
                response
            }
            Err(_) => {
                let _ = response.push_str("{\"error\":\"usb start failed\"}");
                response
            }
        }
    } else if cmd.eq_ignore_ascii_case("USB_UNREGISTER") {
        USB_READY.store(false, core::sync::atomic::Ordering::SeqCst);
        match usb_supervisor::stop(150).await {
            Ok(()) => {
                let _ = response.push_str("{\"ok\":true}");
                response
            }
            Err(_) => {
                let _ = response.push_str("{\"error\":\"usb stop failed\"}");
                response
            }
        }
    } else if let Some(hex) = cmd.strip_prefix("SCRIPT_RUN_HEX ") {
        match decode_hex(hex.trim()) {
            Ok(program) => match HID_CHAN.try_send(HidCommand::RunBytecode { program }) {
                Ok(()) => {
                    let _ = response.push_str("{\"ok\":true,\"queued\":true}");
                    response
                }
                Err(_) => {
                    let _ = response.push_str("{\"error\":\"busy\"}");
                    response
                }
            },
            Err(_) => {
                let _ = response.push_str("{\"error\":\"bad bytecode\"}");
                response
            }
        }
    } else if cmd.starts_with("TRANSFER_START ") || cmd.starts_with("TRANSFER_DEFAULT_SET ") {
        let _ = response.push_str("{\"error\":\"secure binary transfer session required\"}");
        response
    } else if let Some(rest) = cmd.strip_prefix("FS_LIST ") {
        let mut request_id = None;
        let mut cursor = None;
        let mut entry_limit = None;
        let mut flags = None;
        let mut path = None;
        for pair in rest.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                match key {
                    "request_id" => request_id = value.parse::<u64>().ok(),
                    "cursor" => cursor = value.parse::<u32>().ok(),
                    "limit" => entry_limit = value.parse::<u16>().ok(),
                    "flags" => flags = value.parse::<u8>().ok(),
                    "path" => {
                        path = percent_decode_str::<{ MAX_TRANSFER_PATH_LEN }>(value);
                    }
                    _ => {}
                }
            }
        }

        let (Some(request_id), Some(cursor), Some(entry_limit), Some(flags), Some(path)) =
            (request_id, cursor, entry_limit, flags, path)
        else {
            let _ = response.push_str("{\"error\":\"invalid filesystem request\"}");
            return response;
        };

        match CTRL_CHAN.try_send(CtrlCommand::ListDirectory {
            request_id,
            cursor,
            entry_limit,
            flags,
            path,
        }) {
            Ok(()) => {
                let _ = response.push_str("{\"ok\":true,\"queued\":true}");
                response
            }
            Err(_) => {
                let _ = response.push_str("{\"error\":\"busy\"}");
                response
            }
        }
    } else if let Some(rest) = cmd.strip_prefix("FS_LIST_CANCEL ") {
        let request_id = rest
            .split('&')
            .find_map(|pair| pair.strip_prefix("request_id="))
            .and_then(|value| value.parse::<u64>().ok());
        let Some(request_id) = request_id else {
            let _ = response.push_str("{\"error\":\"invalid filesystem request id\"}");
            return response;
        };
        match CTRL_CHAN.try_send(CtrlCommand::CancelDirectoryList { request_id }) {
            Ok(()) => {
                let _ = response.push_str("{\"ok\":true,\"queued\":true}");
                response
            }
            Err(_) => {
                let _ = response.push_str("{\"error\":\"busy\"}");
                response
            }
        }
    } else if cmd.eq_ignore_ascii_case("TRANSFER_START_DEFAULT") {
        let _ = response.push_str("{\"error\":\"secure browser session required\"}");
        response
    } else if cmd.eq_ignore_ascii_case("TRANSFER_MODE_GET") {
        let mode = match crate::usb::ctrl::transfer_relay_mode() {
            crate::usb::ctrl::TransferRelayMode::RelayToBrowser => "relay",
            crate::usb::ctrl::TransferRelayMode::SimulationDrop => "simulation",
        };
        let mut body: String<96> = String::new();
        let _ = core::fmt::write(
            &mut body,
            format_args!(
                "{{\"mode\":\"{}\",\"ws_clients\":{}}}",
                mode,
                ACTIVE_WS_CLIENTS.load(Ordering::Acquire)
            ),
        );
        let _ = response.push_str(body.as_str());
        response
    } else if let Some(rest) = cmd.strip_prefix("TRANSFER_MODE_SET ") {
        let mut mode = None;
        for pair in rest.split('&') {
            if let Some((k, v)) = pair.split_once('=')
                && k == "mode"
            {
                mode = Some(v);
            }
        }
        match mode {
            Some("relay") => {
                crate::usb::ctrl::set_transfer_relay_mode(
                    crate::usb::ctrl::TransferRelayMode::RelayToBrowser,
                );
                let _ = response.push_str("{\"ok\":true,\"mode\":\"relay\"}");
                response
            }
            Some("simulation") => {
                let _ = response.push_str(
                    "{\"error\":\"simulation is unavailable for authenticated transfers\"}",
                );
                response
            }
            _ => {
                let _ = response.push_str("{\"error\":\"bad mode\"}");
                response
            }
        }
    } else {
        // Unknown command
        let _ = response.push_str("{\"error\":\"unknown command\"}");
        response
    }
}

fn decode_hex(input: &str) -> Result<Vec<u8, { MAX_BYTECODE }>, ()> {
    let trimmed = input.trim();
    if trimmed.is_empty() || !trimmed.len().is_multiple_of(2) {
        return Err(());
    }
    let max_bytes = trimmed.len() / 2;
    if max_bytes > MAX_BYTECODE {
        return Err(());
    }
    let mut out = Vec::<u8, { MAX_BYTECODE }>::new();
    let bytes = trimmed.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        let hi = decode_nibble(bytes[idx])?;
        let lo = decode_nibble(bytes[idx + 1])?;
        out.push((hi << 4) | lo).map_err(|_| ())?;
        idx += 2;
    }
    Ok(out)
}

fn decode_nibble(ch: u8) -> Result<u8, ()> {
    match ch {
        b'0'..=b'9' => Ok(ch - b'0'),
        b'a'..=b'f' => Ok(10 + ch - b'a'),
        b'A'..=b'F' => Ok(10 + ch - b'A'),
        _ => Err(()),
    }
}
