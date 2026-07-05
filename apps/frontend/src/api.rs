use futures_channel::oneshot;
use js_sys::{ArrayBuffer, Uint8Array};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

#[derive(Debug, Deserialize)]
struct RpcResponseEnvelope {
    event_type: String,
    request_id: u64,
    payload: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct EventTypeEnvelope {
    event_type: String,
}

// ----- WebSocket state -----

thread_local! {
    static WS: RefCell<Option<WsState>> = RefCell::new(None);
    static NEXT_FILESYSTEM_REQUEST_ID: Cell<u64> = const { Cell::new(1) };
}

struct WsState {
    ws: WebSocket,
    next_request_id: u64,
    pending: HashMap<u64, oneshot::Sender<String>>,
    // Keep event closures alive
    _onopen: Closure<dyn FnMut(Event)>,
    _onmessage: Closure<dyn FnMut(MessageEvent)>,
    _onerror: Closure<dyn FnMut(Event)>,
    _onclose: Closure<dyn FnMut(CloseEvent)>,
}

fn hostname() -> String {
    let window = web_sys::window().expect("window");
    window
        .location()
        .hostname()
        .unwrap_or_else(|_| "192.168.4.1".into())
}

pub fn init_ws(
    on_log: impl Fn(String) + 'static,
    on_state: impl Fn(bool) + 'static,
    on_transfer_text: impl Fn(String) + 'static,
    on_transfer_binary: impl Fn(Vec<u8>) + 'static,
    on_filesystem_binary: impl Fn(Vec<u8>) + 'static,
) {
    WS.with(|cell| {
        if cell.borrow().is_some() {
            return;
        }
        let url = format!("ws://{}/ws", hostname());
        let ws = WebSocket::new(&url).expect("ws connect");
        ws.set_binary_type(BinaryType::Arraybuffer);

        let on_log = Rc::new(on_log);
        let on_state = Rc::new(on_state);
        let on_transfer_text = Rc::new(on_transfer_text);
        let on_transfer_binary = Rc::new(on_transfer_binary);
        let on_filesystem_binary = Rc::new(on_filesystem_binary);

        let onopen = {
            let on_log = on_log.clone();
            let on_state = on_state.clone();
            Closure::wrap(Box::new(move |_e: Event| {
                on_log("WS open".into());
                on_state(true);
            }) as Box<dyn FnMut(_)>)
        };

        let onmessage = {
            let on_log = on_log.clone();
            let on_transfer_text = on_transfer_text.clone();
            let on_transfer_binary = on_transfer_binary.clone();
            let on_filesystem_binary = on_filesystem_binary.clone();
            Closure::wrap(Box::new(move |e: MessageEvent| {
                if let Some(s) = e.data().as_string() {
                    if route_rpc_response(&s) {
                        return;
                    }
                    if is_transfer_event(&s) {
                        on_transfer_text(s);
                        return;
                    }
                    if route_legacy_response(&s) {
                        return;
                    }
                    on_log(format!("WS msg: {}", s));
                    return;
                }

                if let Ok(array_buffer) = e.data().dyn_into::<ArrayBuffer>() {
                    let bytes = Uint8Array::new(&array_buffer);
                    let mut payload = vec![0u8; bytes.length() as usize];
                    bytes.copy_to(&mut payload);
                    match payload.split_first() {
                        Some((1, data)) => on_transfer_binary(data.to_vec()),
                        Some((2, data)) => on_filesystem_binary(data.to_vec()),
                        Some((kind, _)) => {
                            on_log(format!("WS msg: unknown binary kind {kind}"));
                        }
                        None => on_log("WS msg: empty binary message".into()),
                    }
                    return;
                }

                on_log("WS msg: (non-text/non-binary)".into());
            }) as Box<dyn FnMut(_)>)
        };

        let onerror = {
            let on_log = on_log.clone();
            let on_state = on_state.clone();
            Closure::wrap(Box::new(move |_e: Event| {
                let canceled = cancel_pending_requests();
                if canceled > 0 {
                    on_log(format!("WS error; canceled {canceled} pending request(s)"));
                } else {
                    on_log("WS error".into());
                }
                on_state(false);
            }) as Box<dyn FnMut(_)>)
        };
        let onclose = {
            let on_log = on_log.clone();
            let on_state = on_state.clone();
            Closure::wrap(Box::new(move |_e: CloseEvent| {
                let canceled = cancel_pending_requests();
                if canceled > 0 {
                    on_log(format!("WS closed; canceled {canceled} pending request(s)"));
                } else {
                    on_log("WS closed".into());
                }
                on_state(false);
            }) as Box<dyn FnMut(_)>)
        };

        ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));

        cell.replace(Some(WsState {
            ws,
            next_request_id: 1,
            pending: HashMap::new(),
            _onopen: onopen,
            _onmessage: onmessage,
            _onerror: onerror,
            _onclose: onclose,
        }));
    });
}

fn route_rpc_response(text: &str) -> bool {
    let Ok(envelope) = serde_json::from_str::<RpcResponseEnvelope>(text) else {
        return false;
    };
    if envelope.event_type != "command/response" {
        return false;
    }

    let payload = match envelope.payload {
        serde_json::Value::String(value) => value,
        other => other.to_string(),
    };

    WS.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return false;
        };
        let Some(tx) = state.pending.remove(&envelope.request_id) else {
            return false;
        };
        let _ = tx.send(payload);
        true
    })
}

fn route_legacy_response(text: &str) -> bool {
    WS.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return false;
        };
        let first_id = state.pending.keys().next().copied();
        let Some(first_id) = first_id else {
            return false;
        };
        let Some(tx) = state.pending.remove(&first_id) else {
            return false;
        };
        let _ = tx.send(text.to_string());
        true
    })
}

fn is_transfer_event(text: &str) -> bool {
    let Ok(event) = serde_json::from_str::<EventTypeEnvelope>(text) else {
        return false;
    };
    event.event_type.starts_with("transfer/")
}

fn cancel_pending_requests() -> usize {
    WS.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return 0;
        };
        let pending = std::mem::take(&mut state.pending);
        let canceled = pending.len();
        drop(pending);
        canceled
    })
}

async fn send_cmd(cmd: &str) -> Result<String, String> {
    let (tx, rx) = oneshot::channel::<String>();
    let mut tx = Some(tx);
    let mut error = None::<String>;

    WS.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(state) = slot.as_mut() else {
            error = Some("ws not connected".into());
            return;
        };

        let request_id = state.next_request_id;
        state.next_request_id = state.next_request_id.saturating_add(1);

        let Some(tx_value) = tx.take() else {
            error = Some("internal request state error".into());
            return;
        };

        state.pending.insert(request_id, tx_value);

        let message = format!("RPC {} {}", request_id, cmd);
        if state.ws.send_with_str(&message).is_err() {
            state.pending.remove(&request_id);
            error = Some("ws send failed".into());
        }
    });

    if let Some(error) = error {
        return Err(error);
    }

    rx.await.map_err(|_| "ws response canceled".into())
}

pub async fn get_status() -> Result<Status, String> {
    let text = send_cmd("STATUS").await?;
    serde_json::from_str::<Status>(&text).map_err(|e| format!("parse status: {e}: {text}"))
}

pub async fn get_config() -> Result<Config, String> {
    let text = send_cmd("CONFIG_GET").await?;
    serde_json::from_str::<Config>(&text).map_err(|e| format!("parse config: {e}: {text}"))
}

pub async fn save_config(manufacturer: &str, product: &str) -> Result<(), String> {
    let m = utf8_percent_encode(manufacturer, NON_ALPHANUMERIC).to_string();
    let p = utf8_percent_encode(product, NON_ALPHANUMERIC).to_string();
    let cmd = format!("CONFIG_SET manufacturer={}&product={}", m, p);
    let text = send_cmd(&cmd).await?;
    if text.contains("\"ok\":true") {
        Ok(())
    } else {
        Err(text)
    }
}

pub async fn usb_register(assistant: bool, os: Option<&str>) -> Result<(), String> {
    let mut cmd = String::from("USB_REGISTER");
    let mut params: Vec<String> = Vec::new();
    if assistant {
        params.push("assistant=1".to_string());
    }
    if let Some(os) = os {
        if !os.is_empty() {
            params.push(format!("os={}", os));
        }
    }
    if let Some(first) = params.first() {
        cmd.push(' ');
        cmd.push_str(first);
        for extra in params.iter().skip(1) {
            cmd.push('&');
            cmd.push_str(extra);
        }
    }
    let text = send_cmd(&cmd).await?;
    if text.contains("\"ok\":true") {
        Ok(())
    } else {
        Err(text)
    }
}

pub async fn usb_unregister() -> Result<(), String> {
    let text = send_cmd("USB_UNREGISTER").await?;
    if text.contains("\"ok\":true") {
        Ok(())
    } else {
        Err(text)
    }
}

pub async fn list_scripts() -> Result<Vec<ScriptMeta>, String> {
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

pub async fn run_script(bytecode: &[u8]) -> Result<(), String> {
    let encoded = codec::encode_hex(bytecode);
    let cmd = format!("SCRIPT_RUN_HEX {}", encoded);
    let text = send_cmd(&cmd).await?;
    if text.contains("\"ok\":true") {
        Ok(())
    } else {
        Err(text)
    }
}

pub async fn transfer_start(path: &str) -> Result<(), String> {
    let encoded_path = utf8_percent_encode(path, NON_ALPHANUMERIC).to_string();
    let cmd = format!("TRANSFER_START path={encoded_path}");
    let text = send_cmd(&cmd).await?;
    if text.contains("\"ok\":true") {
        Ok(())
    } else {
        Err(text)
    }
}

pub async fn transfer_set_default(path: &str) -> Result<(), String> {
    let encoded_path = utf8_percent_encode(path, NON_ALPHANUMERIC).to_string();
    let cmd = format!("TRANSFER_DEFAULT_SET path={encoded_path}");
    let text = send_cmd(&cmd).await?;
    if text.contains("\"ok\":true") {
        Ok(())
    } else {
        Err(text)
    }
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
) -> Result<(), String> {
    let encoded_path = utf8_percent_encode(path, NON_ALPHANUMERIC).to_string();
    let flags = u8::from(show_hidden);
    let command = format!(
        "FS_LIST request_id={request_id}&cursor={cursor}&limit={entry_limit}&flags={flags}&path={encoded_path}"
    );
    let text = send_cmd(&command).await?;
    if text.contains("\"ok\":true") {
        Ok(())
    } else {
        Err(text)
    }
}

pub async fn filesystem_cancel(request_id: u64) -> Result<(), String> {
    let text = send_cmd(&format!("FS_LIST_CANCEL request_id={request_id}")).await?;
    if text.contains("\"ok\":true") {
        Ok(())
    } else {
        Err(text)
    }
}
