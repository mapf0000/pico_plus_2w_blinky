use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};
use web_sys::{MessageEvent, WebSocket, Event, CloseEvent};
use wasm_bindgen::{closure::Closure, JsCast};
use futures_channel::oneshot;
use std::{cell::RefCell, rc::Rc};
use std::thread_local;

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

// ----- WebSocket state -----

thread_local! {
    static WS: RefCell<Option<WsState>> = RefCell::new(None);
}

struct WsState {
    ws: WebSocket,
    pending: Option<oneshot::Sender<String>>, // single in-flight request
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

pub fn init_ws(on_log: impl Fn(String) + 'static, on_state: impl Fn(bool) + 'static) {
    WS.with(|cell| {
        if cell.borrow().is_some() { return; }
        let url = format!("ws://{}/ws", hostname());
        let ws = WebSocket::new(&url).expect("ws connect");

        let on_log = Rc::new(on_log);
        let on_state = Rc::new(on_state);
        let onopen = {
            let on_log = on_log.clone();
            let on_state = on_state.clone();
            Closure::wrap(Box::new(move |_e: Event| {
                on_log("WS open".into());
                on_state(true);
            }) as Box<dyn FnMut(_)> )
        };

        let onmessage = {
            let on_log = on_log.clone();
            Closure::wrap(Box::new(move |e: MessageEvent| {
                let s = e.data().as_string().unwrap_or_else(|| "(non-text)".into());
                // Try to fulfill pending request first; otherwise log
                let mut delivered = false;
                WS.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        if let Some(tx) = state.pending.take() {
                            let _ = tx.send(s.clone());
                            delivered = true;
                        }
                    }
                });
                if !delivered { on_log(format!("WS msg: {}", s)); }
            }) as Box<dyn FnMut(_)> )
        };

        let onerror = {
            let on_log = on_log.clone();
            let on_state = on_state.clone();
            Closure::wrap(Box::new(move |_e: Event| {
                on_log("WS error".into());
                on_state(false);
            }) as Box<dyn FnMut(_)>)
        };
        let onclose = {
            let on_log = on_log.clone();
            let on_state = on_state.clone();
            Closure::wrap(Box::new(move |_e: CloseEvent| {
                on_log("WS closed".into());
                on_state(false);
            }) as Box<dyn FnMut(_)>)
        };

        ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));

        cell.replace(Some(WsState { ws, pending: None, _onopen: onopen, _onmessage: onmessage, _onerror: onerror, _onclose: onclose }));
    });
}

async fn send_cmd(cmd: &str) -> Result<String, String> {
    // Serialize requests: only one pending at a time
    let (tx, rx) = oneshot::channel::<String>();
    let mut ok = false;
    WS.with(|cell| {
        if let Some(state) = cell.borrow_mut().as_mut() {
            if state.pending.is_none() {
                state.pending = Some(tx);
                if let Err(_e) = state.ws.send_with_str(cmd) {
                    // will error below when awaiting rx
                } else {
                    ok = true;
                }
            }
        }
    });
    if !ok { return Err("ws not connected or busy".into()); }
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
    if text.contains("\"ok\":true") { Ok(()) } else { Err(text) }
}

pub async fn usb_register(_assistant: bool, os: Option<&str>) -> Result<(), String> {
    let mut cmd = String::from("USB_REGISTER");
    if let Some(os) = os { if !os.is_empty() { cmd.push(' '); cmd.push_str(&format!("os={}", os)); } }
    let text = send_cmd(&cmd).await?;
    if text.contains("\"ok\":true") { Ok(()) } else { Err(text) }
}

pub async fn usb_unregister() -> Result<(), String> {
    let text = send_cmd("USB_UNREGISTER").await?;
    if text.contains("\"ok\":true") { Ok(()) } else { Err(text) }
}

pub async fn list_scripts() -> Result<Vec<ScriptMeta>, String> {
    let text = send_cmd("SCRIPTS_LIST").await?;
    serde_json::from_str::<Vec<ScriptMeta>>(&text)
        .map_err(|e| format!("parse scripts: {e}: {} chars", text.len()))
}

pub async fn run_script(dsl: &str) -> Result<(), String> {
    let cmd = format!("SCRIPT_RUN {}", dsl);
    let text = send_cmd(&cmd).await?;
    if text.contains("\"ok\":true") { Ok(()) } else { Err(text) }
}
