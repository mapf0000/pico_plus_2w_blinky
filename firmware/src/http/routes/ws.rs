use embassy_futures::select::{Either as SelectEither, Either4 as SelectEither4, select, select4};
use picoserve::futures::Either;
use picoserve::io::embedded_io_async;
use picoserve::response::ws; // for Read/Write trait bounds

use crate::host::{self, HostOs};
use crate::http::transfer::{self, TRANSFER_TEXT_MAX};
use crate::http::util::{escape_json_str, percent_decode_str};
use crate::usb::ctrl::{CTRL_CHAN, CtrlCommand, MAX_SECURE_TRANSFER_FRAME, MAX_TRANSFER_PATH_LEN};
use crate::usb::hid::{
    HID_CANCEL, HID_CHAN, HID_RESULT_CHAN, HidCancel, HidCommand, HidResult, HidResultStatus,
    MAX_BYTECODE, USB_READY,
};
use crate::usb::usb_supervisor;
use heapless::{String, Vec};

pub const WS_BINARY_KIND_SESSION: u8 = transfer_protocol::WS_BINARY_KIND_SESSION;
const WS_COMMAND_MAX: usize = script_protocol::MAX_MESSAGE_LEN;

pub(crate) async fn ws_handler(
    upgrade: ws::WebSocketUpgrade,
) -> impl picoserve::response::IntoResponse {
    crate::health::mark(crate::health::Stage::WebSocketUpgrade);
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
        crate::health::mark(crate::health::Stage::WebSocketHello);
        let hello = crate::capabilities::hello_json();
        tx.send_text(hello.as_str()).await?;
        crate::health::mark(crate::health::Stage::WebSocketActive);
        let session = transfer::begin_session();
        let _ = CTRL_CHAN.try_send(CtrlCommand::RequestStatus);

        let mut buf = [0u8; WS_COMMAND_MAX];
        let mut exit = ConnectionExit::Peer;
        loop {
            match select4(
                session.replaced(),
                rx.next_message(&mut buf, core::future::pending::<()>()),
                transfer::receive_frame(&session),
                HID_RESULT_CHAN.receive(),
            )
            .await
            {
                // Replacement is polled first, then browser control traffic,
                // then bulk output. A saturated transfer cannot delay handoff.
                SelectEither4::First(()) => {
                    exit = ConnectionExit::Replaced;
                    break;
                }
                SelectEither4::Second(result) => match result {
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
                            match send_text_unless_replaced(&session, &mut tx, envelope.as_str())
                                .await
                            {
                                Ok(false) => {}
                                Ok(true) => {
                                    exit = ConnectionExit::Replaced;
                                    break;
                                }
                                Err(_) => break,
                            }
                            continue;
                        }

                        let response = handle_command(cmd).await;
                        match send_text_unless_replaced(&session, &mut tx, response.as_str()).await
                        {
                            Ok(false) => {}
                            Ok(true) => {
                                exit = ConnectionExit::Replaced;
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                    Ok(Either::First(Ok(ws::Message::Binary(binary)))) => {
                        let Some((&kind, body)) = binary.split_first() else {
                            continue;
                        };
                        if kind == WS_BINARY_KIND_SESSION {
                            if body.len() > MAX_SECURE_TRANSFER_FRAME {
                                continue;
                            }
                            let mut payload: Vec<u8, MAX_SECURE_TRANSFER_FRAME> = Vec::new();
                            if payload.extend_from_slice(body).is_ok() {
                                let _ = CTRL_CHAN.try_send(CtrlCommand::SecureTransfer { payload });
                            }
                            continue;
                        }
                        if kind == script_protocol::WS_BINARY_KIND_SCRIPT_EFFECT {
                            let result = handle_script_binary(binary);
                            if let Some(result) = result {
                                match send_hid_result_unless_replaced(&session, &mut tx, result)
                                    .await
                                {
                                    Ok(false) => {}
                                    Ok(true) => {
                                        exit = ConnectionExit::Replaced;
                                        break;
                                    }
                                    Err(_) => break,
                                }
                            }
                        }
                    }
                    Ok(Either::First(Ok(ws::Message::Ping(p)))) => {
                        match send_pong_unless_replaced(&session, &mut tx, p).await {
                            Ok(false) => {}
                            Ok(true) => {
                                exit = ConnectionExit::Replaced;
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                    Ok(Either::First(Ok(ws::Message::Pong(_)))) => {}
                    Ok(Either::First(Ok(ws::Message::Close(_)))) => break,
                    Ok(Either::First(Err(_))) => break,
                    Ok(Either::Second(_)) => break,
                    Err(_) => break,
                },
                SelectEither4::Third(frame) => {
                    match send_frame_unless_replaced(&session, frame, &mut tx).await {
                        Ok(false) => {}
                        Ok(true) => {
                            exit = ConnectionExit::Replaced;
                            break;
                        }
                        Err(_) => break,
                    }
                }
                SelectEither4::Fourth(result) => {
                    match send_hid_result_unless_replaced(&session, &mut tx, result).await {
                        Ok(false) => {}
                        Ok(true) => {
                            exit = ConnectionExit::Replaced;
                            break;
                        }
                        Err(_) => break,
                    }
                }
            }
        }

        drop(session);
        match exit {
            ConnectionExit::Peer => tx.close(None).await,
            ConnectionExit::Replaced => {
                log::info!("websocket: closing replaced browser session");
                tx.close(Some((
                    transfer_protocol::WEBSOCKET_CLOSE_SESSION_REPLACED,
                    "session replaced",
                )))
                .await
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ConnectionExit {
    Peer,
    Replaced,
}

async fn send_text_unless_replaced<W: embedded_io_async::Write>(
    session: &transfer::SessionGuard,
    tx: &mut ws::SocketTx<W>,
    text: &str,
) -> Result<bool, W::Error> {
    match select(tx.send_text(text), session.replaced()).await {
        SelectEither::First(result) => result.map(|()| false),
        SelectEither::Second(()) => Ok(true),
    }
}

async fn send_pong_unless_replaced<W: embedded_io_async::Write>(
    session: &transfer::SessionGuard,
    tx: &mut ws::SocketTx<W>,
    payload: &[u8],
) -> Result<bool, W::Error> {
    match select(tx.send_pong(payload), session.replaced()).await {
        SelectEither::First(result) => result.map(|()| false),
        SelectEither::Second(()) => Ok(true),
    }
}

async fn send_frame_unless_replaced<W: embedded_io_async::Write>(
    session: &transfer::SessionGuard,
    frame: transfer::OutboundFrame,
    tx: &mut ws::SocketTx<W>,
) -> Result<bool, W::Error> {
    // Poll send_frame first so a batch frame installs its RAII lease before a
    // simultaneous replacement can cancel the write and return the buffer.
    match select(transfer::send_frame(frame, tx), session.replaced()).await {
        SelectEither::First(result) => result.map(|()| false),
        SelectEither::Second(()) => Ok(true),
    }
}

fn handle_script_binary(frame: &[u8]) -> Option<HidResult> {
    match script_protocol::decode(frame).ok()? {
        script_protocol::Message::Run { id, bytecode } => {
            if !USB_READY.load(core::sync::atomic::Ordering::SeqCst) {
                return Some(HidResult {
                    request_id: id.request_id,
                    process_id: id.process_id,
                    effect_id: id.effect_id,
                    status: HidResultStatus::UsbUnavailable,
                });
            }
            let mut program: Vec<u8, { MAX_BYTECODE }> = Vec::new();
            if program.extend_from_slice(bytecode).is_err()
                || firmware_exec::validate_bytecode(program.as_slice()).is_err()
            {
                return Some(HidResult {
                    request_id: id.request_id,
                    process_id: id.process_id,
                    effect_id: id.effect_id,
                    status: HidResultStatus::Rejected,
                });
            }
            match HID_CHAN.try_send(HidCommand::RunEffect {
                request_id: id.request_id,
                process_id: id.process_id,
                effect_id: id.effect_id,
                program,
            }) {
                Ok(()) => None,
                Err(_) => Some(HidResult {
                    request_id: id.request_id,
                    process_id: id.process_id,
                    effect_id: id.effect_id,
                    status: HidResultStatus::Rejected,
                }),
            }
        }
        script_protocol::Message::Cancel { id } => {
            HID_CANCEL.signal(HidCancel {
                process_id: id.process_id,
                effect_id: id.effect_id,
            });
            None
        }
    }
}

async fn send_hid_result_unless_replaced<W: embedded_io_async::Write>(
    session: &transfer::SessionGuard,
    tx: &mut ws::SocketTx<W>,
    result: HidResult,
) -> Result<bool, W::Error> {
    let status = match result.status {
        HidResultStatus::Completed => "completed",
        HidResultStatus::Rejected => "rejected",
        HidResultStatus::Cancelled => "cancelled",
        HidResultStatus::UsbUnavailable => "usb_unavailable",
    };
    let mut event: String<TRANSFER_TEXT_MAX> = String::new();
    let _ = core::fmt::write(
        &mut event,
        format_args!(
            concat!(
                "{{\"event_type\":\"script/effect_result\",\"version\":1,",
                "\"request_id\":\"{:016x}\",\"process_id\":\"{:016x}\",",
                "\"effect_id\":\"{:016x}\",\"status\":\"{}\"}}"
            ),
            result.request_id, result.process_id, result.effect_id, status,
        ),
    );
    send_text_unless_replaced(session, tx, event.as_str()).await
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
        let host_agent = crate::capabilities::host_agent_snapshot();
        let host_os = if host_agent.present && !host_agent.host_os.is_empty() {
            host_agent.host_os.as_str()
        } else {
            host::host_os_str()
        };
        let mut body: String<96> = String::new();
        let _ = core::fmt::write(
            &mut body,
            format_args!(
                "{{\"usb_enabled\":{},\"usb_ready\":{},\"host_os\":\"{}\"}}",
                enabled, ready, host_os
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
        // optional: USB_REGISTER os=mac
        if let Some(q) = cmd.strip_prefix("USB_REGISTER ") {
            for pair in q.split('&') {
                if let Some((k, v)) = pair.split_once('=')
                    && k == "os"
                {
                    match v {
                        "mac" => host::set_host_os(HostOs::Mac),
                        "windows" => host::set_host_os(HostOs::Windows),
                        _ => host::set_host_os(HostOs::Unknown),
                    }
                }
            }
        }
        match usb_supervisor::start().await {
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
                transfer::active_client_count()
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
