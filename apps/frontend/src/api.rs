use futures_channel::oneshot;
use gloo_timers::callback::Timeout;
use js_sys::{ArrayBuffer, Uint8Array};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::thread_local;
use std::vec::Vec;
use std::{cell::Cell, cell::RefCell, rc::Rc};
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{BinaryType, CloseEvent, Event, MessageEvent, WebSocket};

use crate::{codec, scripts};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Status {
    pub usb_enabled: bool,
    pub usb_ready: bool,
    pub host_os: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Config {
    pub usb_manufacturer: String,
    pub usb_product: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ScriptMeta {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub dsl: Option<String>,
}

pub const WEBSOCKET_PROTOCOL_VERSION: u16 = 2;
pub const TRANSFER_PROTOCOL_VERSION: u16 = transfer_protocol::TRANSFER_PROTOCOL_VERSION;
pub const FILESYSTEM_PROTOCOL_VERSION: u16 = 1;

const READ_TIMEOUT_MS: u32 = 5_000;
const MUTATION_TIMEOUT_MS: u32 = 10_000;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Hello {
    pub event_type: String,
    pub version: u16,
    pub firmware: FirmwareBuild,
    pub protocols: ProtocolVersions,
    pub host_agent: HostAgentInfo,
    pub keyboard: KeyboardCapabilities,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub privileged_operations: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct FirmwareBuild {
    pub version: String,
    pub build: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ProtocolVersions {
    pub websocket: u16,
    pub transfer: u16,
    pub filesystem: u16,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct HostAgentInfo {
    pub present: bool,
    pub version: Option<String>,
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct KeyboardCapabilities {
    #[serde(default)]
    pub layouts: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
}

impl Hello {
    pub fn compatibility_error(&self) -> Option<String> {
        if self.event_type != "hello" || self.version != 1 {
            return Some("unsupported HELLO format".into());
        }
        if self.protocols.websocket != WEBSOCKET_PROTOCOL_VERSION {
            return Some(format!(
                "WebSocket protocol {} is not supported by this UI (expected {})",
                self.protocols.websocket, WEBSOCKET_PROTOCOL_VERSION
            ));
        }
        None
    }

    pub fn supports_feature(&self, feature: &str) -> bool {
        self.features.iter().any(|candidate| candidate == feature)
    }

    pub fn supports_keyboard_feature(&self, feature: &str) -> bool {
        self.keyboard
            .features
            .iter()
            .any(|candidate| candidate == feature)
    }

    pub fn supports_layout(&self, layout: &str) -> bool {
        self.keyboard
            .layouts
            .iter()
            .any(|candidate| candidate == layout)
    }

    pub fn transfer_compatible(&self) -> bool {
        self.protocols.transfer == TRANSFER_PROTOCOL_VERSION
            && self.supports_feature("file_transfer")
    }

    pub fn filesystem_compatible(&self) -> bool {
        self.protocols.filesystem == FILESYSTEM_PROTOCOL_VERSION
            && self.supports_feature("filesystem_browser")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    ConnectionLost,
    Reconnecting,
}

impl fmt::Display for CancelReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConnectionLost => f.write_str("connection lost"),
            Self::Reconnecting => f.write_str("connection replaced while reconnecting"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    Disconnected,
    SendFailed,
    Timeout {
        request_id: u64,
        timeout_ms: u32,
    },
    Canceled {
        request_id: u64,
        reason: CancelReason,
    },
    Protocol(String),
    Remote(String),
}

impl ApiError {
    pub fn category(&self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::SendFailed => "transport",
            Self::Timeout { .. } => "timeout",
            Self::Canceled { .. } => "canceled",
            Self::Protocol(_) => "protocol",
            Self::Remote(_) => "remote",
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disconnected => f.write_str("device is not connected"),
            Self::SendFailed => f.write_str("failed to send request"),
            Self::Timeout {
                request_id,
                timeout_ms,
            } => write!(f, "request {request_id} timed out after {timeout_ms} ms"),
            Self::Canceled { request_id, reason } => {
                write!(f, "request {request_id} canceled: {reason}")
            }
            Self::Protocol(message) | Self::Remote(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug, Deserialize)]
struct RpcResponseEnvelope {
    event_type: String,
    version: u16,
    request_id: u64,
    payload: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct EventTypeEnvelope {
    event_type: String,
}

// ----- WebSocket state -----

thread_local! {
    static WS: RefCell<Option<WsState>> = const { RefCell::new(None) };
    static WS_CALLBACKS: RefCell<Option<WsCallbacks>> = const { RefCell::new(None) };
    static RECONNECT_SCHEDULED: Cell<bool> = const { Cell::new(false) };
    static RECONNECT_DELAY_MS: Cell<u32> = const { Cell::new(RECONNECT_DELAY_MIN_MS) };
    static NEXT_FILESYSTEM_REQUEST_ID: Cell<u64> = const { Cell::new(1) };
    static NEXT_WS_SESSION_ID: Cell<u64> = const { Cell::new(1) };
}

const RECONNECT_DELAY_MIN_MS: u32 = 500;
const RECONNECT_DELAY_MAX_MS: u32 = 5_000;

#[derive(Clone)]
struct WsCallbacks {
    on_log: Rc<dyn Fn(String)>,
    on_state: Rc<dyn Fn(bool)>,
    on_hello: Rc<dyn Fn(Hello)>,
    on_transfer_text: Rc<dyn Fn(String)>,
    on_secure_transfer_binary: Rc<dyn Fn(u8, Vec<u8>)>,
    on_filesystem_binary: Rc<dyn Fn(Vec<u8>)>,
}

struct WsState {
    ws: WebSocket,
    session_id: u64,
    next_request_id: u64,
    pending: HashMap<u64, oneshot::Sender<Result<String, ApiError>>>,
    // Keep event closures alive
    _onopen: Closure<dyn FnMut(Event)>,
    _onmessage: Closure<dyn FnMut(MessageEvent)>,
    _onerror: Closure<dyn FnMut(Event)>,
    _onclose: Closure<dyn FnMut(CloseEvent)>,
}

fn websocket_url() -> String {
    let window = web_sys::window().expect("window");
    let location = window.location();
    let scheme = match location.protocol().as_deref() {
        Ok("https:") => "wss",
        _ => "ws",
    };
    let host = location
        .host()
        .ok()
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "192.168.4.1".into());
    let hostname = location
        .hostname()
        .ok()
        .filter(|hostname| !hostname.is_empty())
        .unwrap_or_else(|| "192.168.4.1".into());
    let page_port = location.port().ok().unwrap_or_default();
    websocket_url_from_parts(scheme, &host, &hostname, &page_port)
}

fn websocket_url_from_parts(scheme: &str, host: &str, hostname: &str, page_port: &str) -> String {
    // Trunk serves the frontend on a non-default development port and proxies
    // `/ws`. The embedded frontend is served on port 80 and connects directly
    // to the singleton WebSocket data plane on port 81.
    if page_port.is_empty() || page_port == "80" {
        format!(
            "{scheme}://{hostname}:{}/ws",
            transfer_protocol::WEBSOCKET_PORT
        )
    } else {
        format!("{scheme}://{host}/ws")
    }
}

pub fn init_ws(
    on_log: impl Fn(String) + 'static,
    on_state: impl Fn(bool) + 'static,
    on_hello: impl Fn(Hello) + 'static,
    on_transfer_text: impl Fn(String) + 'static,
    on_secure_transfer_binary: impl Fn(u8, Vec<u8>) + 'static,
    on_filesystem_binary: impl Fn(Vec<u8>) + 'static,
) {
    WS_CALLBACKS.with(|cell| {
        cell.replace(Some(WsCallbacks {
            on_log: Rc::new(on_log),
            on_state: Rc::new(on_state),
            on_hello: Rc::new(on_hello),
            on_transfer_text: Rc::new(on_transfer_text),
            on_secure_transfer_binary: Rc::new(on_secure_transfer_binary),
            on_filesystem_binary: Rc::new(on_filesystem_binary),
        }));
    });
    connect_ws();
}

fn connect_ws() {
    let already_connected = WS.with(|cell| {
        cell.borrow().as_ref().is_some_and(|state| {
            matches!(
                state.ws.ready_state(),
                WebSocket::CONNECTING | WebSocket::OPEN
            )
        })
    });
    if already_connected {
        return;
    }

    let Some(callbacks) = WS_CALLBACKS.with(|cell| cell.borrow().clone()) else {
        return;
    };

    cancel_all_pending(CancelReason::Reconnecting);
    let session_id = NEXT_WS_SESSION_ID.with(|next| {
        let value = next.get();
        next.set(value.saturating_add(1));
        value
    });

    let url = websocket_url();
    let Ok(ws) = WebSocket::new(&url) else {
        (callbacks.on_log)("WS connection failed; retrying".into());
        (callbacks.on_state)(false);
        schedule_ws_reconnect();
        return;
    };
    ws.set_binary_type(BinaryType::Arraybuffer);

    let onopen = {
        let callbacks = callbacks.clone();
        Closure::wrap(Box::new(move |_e: Event| {
            if !is_current_session(session_id) {
                return;
            }
            RECONNECT_DELAY_MS.with(|delay| delay.set(RECONNECT_DELAY_MIN_MS));
            (callbacks.on_log)("WS open".into());
            (callbacks.on_state)(true);
        }) as Box<dyn FnMut(_)>)
    };

    let onmessage = {
        let callbacks = callbacks.clone();
        Closure::wrap(Box::new(move |e: MessageEvent| {
            if !is_current_session(session_id) {
                return;
            }
            if let Some(s) = e.data().as_string() {
                if route_rpc_response(&s) {
                    return;
                }
                if let Ok(hello) = serde_json::from_str::<Hello>(&s)
                    && hello.event_type == "hello"
                {
                    (callbacks.on_hello)(hello);
                    return;
                }
                if is_transfer_event(&s) {
                    (callbacks.on_transfer_text)(s);
                    return;
                }
                (callbacks.on_log)(format!("WS msg: {s}"));
                return;
            }

            if let Ok(array_buffer) = e.data().dyn_into::<ArrayBuffer>() {
                let bytes = Uint8Array::new(&array_buffer);
                let mut payload = vec![0u8; bytes.length() as usize];
                bytes.copy_to(&mut payload);
                match payload.split_first() {
                    Some((2, data)) => (callbacks.on_filesystem_binary)(data.to_vec()),
                    Some((kind @ 3..=7, data)) => {
                        (callbacks.on_secure_transfer_binary)(*kind, data.to_vec())
                    }
                    Some((kind, _)) => {
                        (callbacks.on_log)(format!("WS msg: unknown binary kind {kind}"));
                    }
                    None => (callbacks.on_log)("WS msg: empty binary message".into()),
                }
                return;
            }

            (callbacks.on_log)("WS msg: (non-text/non-binary)".into());
        }) as Box<dyn FnMut(_)>)
    };

    let onerror = {
        let callbacks = callbacks.clone();
        Closure::wrap(Box::new(move |_e: Event| {
            if !is_current_session(session_id) {
                return;
            }
            let canceled = cancel_pending_requests(session_id, CancelReason::ConnectionLost);
            if canceled > 0 {
                (callbacks.on_log)(format!(
                    "Connection interrupted; canceled {canceled} pending request(s)"
                ));
            }
            (callbacks.on_state)(false);
            schedule_ws_reconnect();
        }) as Box<dyn FnMut(_)>)
    };
    let onclose = {
        let callbacks = callbacks.clone();
        Closure::wrap(Box::new(move |_e: CloseEvent| {
            if !is_current_session(session_id) {
                return;
            }
            let canceled = cancel_pending_requests(session_id, CancelReason::ConnectionLost);
            let _ = canceled;
            (callbacks.on_log)("Device connection lost; retrying".into());
            (callbacks.on_state)(false);
            schedule_ws_reconnect();
        }) as Box<dyn FnMut(_)>)
    };

    ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
    ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));
    ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));

    WS.with(|cell| {
        cell.replace(Some(WsState {
            ws,
            session_id,
            next_request_id: 1,
            pending: HashMap::new(),
            _onopen: onopen,
            _onmessage: onmessage,
            _onerror: onerror,
            _onclose: onclose,
        }));
    });
}

fn schedule_ws_reconnect() {
    let already_scheduled = RECONNECT_SCHEDULED.with(|scheduled| scheduled.replace(true));
    if already_scheduled {
        return;
    }

    let delay_ms = RECONNECT_DELAY_MS.with(|delay| {
        let current = delay.get();
        delay.set(current.saturating_mul(2).min(RECONNECT_DELAY_MAX_MS));
        current
    });

    Timeout::new(delay_ms, move || {
        RECONNECT_SCHEDULED.with(|scheduled| scheduled.set(false));
        let should_reconnect = clear_disconnected_session();
        if should_reconnect {
            connect_ws();
        }
    })
    .forget();
}

fn clear_disconnected_session() -> bool {
    WS.with(|cell| {
        let mut slot = cell.borrow_mut();
        let disconnected = slot.as_ref().is_none_or(|state| {
            !matches!(
                state.ws.ready_state(),
                WebSocket::CONNECTING | WebSocket::OPEN
            )
        });
        if !disconnected {
            return false;
        }
        if let Some(mut state) = slot.take() {
            for (request_id, tx) in state.pending.drain() {
                let _ = tx.send(Err(ApiError::Canceled {
                    request_id,
                    reason: CancelReason::Reconnecting,
                }));
            }
        }
        true
    })
}

fn route_rpc_response(text: &str) -> bool {
    let Ok(envelope) = serde_json::from_str::<RpcResponseEnvelope>(text) else {
        return false;
    };
    if envelope.event_type != "command/response" {
        return false;
    }

    let response = if envelope.version != WEBSOCKET_PROTOCOL_VERSION {
        Err(ApiError::Protocol(format!(
            "response {} uses unsupported WebSocket protocol version {}",
            envelope.request_id, envelope.version
        )))
    } else {
        Ok(match envelope.payload {
            serde_json::Value::String(value) => value,
            other => other.to_string(),
        })
    };

    WS.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return true;
        };
        let Some(tx) = state.pending.remove(&envelope.request_id) else {
            return true;
        };
        let _ = tx.send(response);
        true
    })
}

fn is_transfer_event(text: &str) -> bool {
    let Ok(event) = serde_json::from_str::<EventTypeEnvelope>(text) else {
        return false;
    };
    event.event_type.starts_with("transfer/")
}

fn is_current_session(session_id: u64) -> bool {
    WS.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_some_and(|state| state.session_id == session_id)
    })
}

fn cancel_pending_requests(session_id: u64, reason: CancelReason) -> usize {
    WS.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return 0;
        };
        if state.session_id != session_id {
            return 0;
        }
        let pending = std::mem::take(&mut state.pending);
        let canceled = pending.len();
        for (request_id, tx) in pending {
            let _ = tx.send(Err(ApiError::Canceled { request_id, reason }));
        }
        canceled
    })
}

fn cancel_all_pending(reason: CancelReason) -> usize {
    let session_id = WS.with(|cell| cell.borrow().as_ref().map(|state| state.session_id));
    session_id.map_or(0, |id| cancel_pending_requests(id, reason))
}

fn timeout_request(session_id: u64, request_id: u64, timeout_ms: u32) {
    WS.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return;
        };
        if state.session_id != session_id {
            return;
        }
        if let Some(tx) = state.pending.remove(&request_id) {
            let _ = tx.send(Err(ApiError::Timeout {
                request_id,
                timeout_ms,
            }));
        }
    });
}

async fn send_cmd(cmd: &str, timeout_ms: u32) -> Result<String, ApiError> {
    let (tx, rx) = oneshot::channel::<Result<String, ApiError>>();
    let mut tx = Some(tx);
    let mut error = None::<ApiError>;
    let mut registered = None::<(u64, u64)>;

    WS.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(state) = slot.as_mut() else {
            error = Some(ApiError::Disconnected);
            return;
        };
        if state.ws.ready_state() != WebSocket::OPEN {
            error = Some(ApiError::Disconnected);
            return;
        };

        let request_id = state.next_request_id;
        state.next_request_id = state.next_request_id.saturating_add(1);

        let Some(tx_value) = tx.take() else {
            error = Some(ApiError::Protocol("internal request state error".into()));
            return;
        };

        state.pending.insert(request_id, tx_value);
        registered = Some((state.session_id, request_id));

        let message = format!("RPC {} {}", request_id, cmd);
        if state.ws.send_with_str(&message).is_err() {
            state.pending.remove(&request_id);
            registered = None;
            error = Some(ApiError::SendFailed);
        }
    });

    if let Some(error) = error {
        return Err(error);
    }

    let Some((session_id, request_id)) = registered else {
        return Err(ApiError::Protocol("request was not registered".into()));
    };
    Timeout::new(timeout_ms, move || {
        timeout_request(session_id, request_id, timeout_ms);
    })
    .forget();

    rx.await.unwrap_or({
        Err(ApiError::Canceled {
            request_id,
            reason: CancelReason::ConnectionLost,
        })
    })
}

fn parse_json<T: for<'de> Deserialize<'de>>(name: &str, text: &str) -> Result<T, ApiError> {
    serde_json::from_str(text)
        .map_err(|error| ApiError::Protocol(format!("parse {name}: {error}: {text}")))
}

fn expect_ok(text: &str) -> Result<(), ApiError> {
    let value: serde_json::Value = parse_json("command response", text)?;
    if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(());
    }
    if let Some(message) = value.get("error").and_then(serde_json::Value::as_str) {
        return Err(ApiError::Remote(message.to_string()));
    }
    Err(ApiError::Protocol(format!(
        "command returned an invalid response: {text}"
    )))
}

pub async fn get_hello() -> Result<Hello, ApiError> {
    let text = send_cmd("HELLO", READ_TIMEOUT_MS).await?;
    parse_json("HELLO", &text)
}

pub async fn get_status() -> Result<Status, ApiError> {
    let text = send_cmd("STATUS", READ_TIMEOUT_MS).await?;
    parse_json("status", &text)
}

pub async fn get_config() -> Result<Config, ApiError> {
    let text = send_cmd("CONFIG_GET", READ_TIMEOUT_MS).await?;
    parse_json("config", &text)
}

pub async fn save_config(manufacturer: &str, product: &str) -> Result<(), ApiError> {
    let m = utf8_percent_encode(manufacturer, NON_ALPHANUMERIC).to_string();
    let p = utf8_percent_encode(product, NON_ALPHANUMERIC).to_string();
    let cmd = format!("CONFIG_SET manufacturer={}&product={}", m, p);
    let text = send_cmd(&cmd, MUTATION_TIMEOUT_MS).await?;
    expect_ok(&text)
}

pub async fn usb_register(assistant: bool, os: Option<&str>) -> Result<(), ApiError> {
    let mut cmd = String::from("USB_REGISTER");
    let mut params: Vec<String> = Vec::new();
    if assistant {
        params.push("assistant=1".to_string());
    }
    if let Some(os) = os
        && !os.is_empty()
    {
        params.push(format!("os={}", os));
    }
    if let Some(first) = params.first() {
        cmd.push(' ');
        cmd.push_str(first);
        for extra in params.iter().skip(1) {
            cmd.push('&');
            cmd.push_str(extra);
        }
    }
    let text = send_cmd(&cmd, MUTATION_TIMEOUT_MS).await?;
    expect_ok(&text)
}

pub async fn usb_unregister() -> Result<(), ApiError> {
    let text = send_cmd("USB_UNREGISTER", MUTATION_TIMEOUT_MS).await?;
    expect_ok(&text)
}

pub async fn list_scripts() -> Result<Vec<ScriptMeta>, ApiError> {
    let list = scripts::all()
        .iter()
        .map(|s| ScriptMeta {
            id: s.id.to_string(),
            name: s.name.to_string(),
            description: s.description.to_string(),
            dsl: Some(s.dsl.to_string()),
        })
        .collect();
    Ok(list)
}

pub async fn run_script(bytecode: &[u8]) -> Result<(), ApiError> {
    let encoded = codec::encode_hex(bytecode);
    let cmd = format!("SCRIPT_RUN_HEX {}", encoded);
    let text = send_cmd(&cmd, MUTATION_TIMEOUT_MS).await?;
    expect_ok(&text)
}

pub fn send_secure_transfer(payload: &[u8]) -> Result<(), ApiError> {
    let mut error = None;
    WS.with(|cell| {
        let slot = cell.borrow();
        let Some(state) = slot.as_ref() else {
            error = Some(ApiError::Disconnected);
            return;
        };
        if state.ws.ready_state() != WebSocket::OPEN {
            error = Some(ApiError::Disconnected);
            return;
        }
        let mut message = Vec::with_capacity(payload.len() + 1);
        message.push(transfer_protocol::WS_BINARY_KIND_SESSION);
        message.extend_from_slice(payload);
        if state.ws.send_with_u8_array(&message).is_err() {
            error = Some(ApiError::SendFailed);
        }
    });
    error.map_or(Ok(()), Err)
}

pub fn next_filesystem_request_id() -> u64 {
    NEXT_FILESYSTEM_REQUEST_ID.with(|next| {
        let request_id = next.get();
        next.set(request_id.saturating_add(1));
        request_id
    })
}

pub async fn filesystem_list(
    request_id: u64,
    path: &str,
    cursor: u32,
    entry_limit: u16,
    show_hidden: bool,
) -> Result<(), ApiError> {
    let encoded_path = utf8_percent_encode(path, NON_ALPHANUMERIC).to_string();
    let flags = u8::from(show_hidden);
    let command = format!(
        "FS_LIST request_id={request_id}&cursor={cursor}&limit={entry_limit}&flags={flags}&path={encoded_path}"
    );
    let text = send_cmd(&command, MUTATION_TIMEOUT_MS).await?;
    expect_ok(&text)
}

pub async fn filesystem_cancel(request_id: u64) -> Result<(), ApiError> {
    let text = send_cmd(
        &format!("FS_LIST_CANCEL request_id={request_id}"),
        MUTATION_TIMEOUT_MS,
    )
    .await?;
    expect_ok(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello(websocket: u16) -> Hello {
        Hello {
            event_type: "hello".into(),
            version: 1,
            firmware: FirmwareBuild {
                version: "0.1.0".into(),
                build: "test".into(),
            },
            protocols: ProtocolVersions {
                websocket,
                transfer: TRANSFER_PROTOCOL_VERSION,
                filesystem: FILESYSTEM_PROTOCOL_VERSION,
            },
            host_agent: HostAgentInfo {
                present: true,
                version: Some("0.1.0".into()),
                hostname: Some("test-host".into()),
            },
            keyboard: KeyboardCapabilities {
                layouts: vec!["mac_de-DE".into()],
                features: vec!["hid_keyboard".into()],
            },
            features: vec!["file_transfer".into(), "filesystem_browser".into()],
            privileged_operations: vec!["host_execute".into()],
        }
    }

    #[test]
    fn hello_rejects_incompatible_websocket_protocol() {
        let error = hello(WEBSOCKET_PROTOCOL_VERSION + 1)
            .compatibility_error()
            .expect("protocol mismatch should be rejected");
        assert!(error.contains("WebSocket protocol"));
    }

    #[test]
    fn embedded_frontend_uses_singleton_websocket_port() {
        assert_eq!(
            websocket_url_from_parts("ws", "192.168.4.1", "192.168.4.1", ""),
            "ws://192.168.4.1:81/ws"
        );
    }

    #[test]
    fn development_frontend_keeps_same_origin_proxy() {
        assert_eq!(
            websocket_url_from_parts("ws", "localhost:8080", "localhost", "8080"),
            "ws://localhost:8080/ws"
        );
    }

    #[test]
    fn keyboard_features_are_read_from_keyboard_capabilities() {
        let mut hello = hello(WEBSOCKET_PROTOCOL_VERSION);
        hello.keyboard.features.push("script_bytecode".into());

        assert!(hello.supports_keyboard_feature("script_bytecode"));
        assert!(!hello.supports_feature("script_bytecode"));
    }

    #[test]
    fn command_errors_are_typed() {
        assert_eq!(
            expect_ok(r#"{"error":"busy"}"#),
            Err(ApiError::Remote("busy".into()))
        );
        assert_eq!(expect_ok("not-json").unwrap_err().category(), "protocol");
    }
}
