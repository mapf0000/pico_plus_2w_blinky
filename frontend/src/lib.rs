use wasm_bindgen::prelude::*;
use yew::prelude::*;

use dsl_core::MAX_DSL_LINES;

mod api;
pub mod codec;
pub mod dsl;
pub mod scripts;

#[derive(Clone, PartialEq)]
struct StatusState {
    usb_enabled: bool,
    usb_ready: bool,
    host_os: String,
}

#[derive(Clone, PartialEq)]
struct ConfigState {
    usb_manufacturer: String,
    usb_product: String,
}

#[function_component(App)]
fn app() -> Html {
    let status: UseStateHandle<Option<StatusState>> = use_state(|| None);
    let config: UseStateHandle<Option<ConfigState>> = use_state(|| None);
    let scripts: UseStateHandle<Option<Vec<api::ScriptMeta>>> = use_state(|| None);
    let dsl_text = use_state(|| String::new());
    let selected_os = use_state(|| String::from("mac"));
    let busy_count = use_state(|| 0u32);
    let log_lines = use_state(|| Vec::<String>::new());
    let ws_connected = use_state(|| false);
    let toast = use_state(|| None::<(String, bool)>); // (message, ok?)
    // All WebSocket API calls are handled in api.rs via a single connection

    let set_busy = {
        let busy_count = busy_count.clone();
        Callback::from(move |on: bool| {
            busy_count.set(busy_count.saturating_add(if on { 1 } else { u32::MAX }));
        })
    };

    let push_log = {
        let log_lines = log_lines.clone();
        Callback::from(move |line: String| {
            let mut v = (*log_lines).clone();
            v.push(line);
            if v.len() > 300 {
                let _ = v.drain(0..v.len() - 300);
            }
            log_lines.set(v);
        })
    };

    let show_toast = {
        let toast = toast.clone();
        Callback::from(move |(msg, ok): (String, bool)| {
            toast.set(Some((msg, ok)));
            let toast = toast.clone();
            gloo_timers::callback::Timeout::new(3000, move || {
                toast.set(None);
            })
            .forget();
        })
    };

    // Initial fetches
    {
        let status = status.clone();
        let config = config.clone();
        let scripts_state = scripts.clone();
        let set_busy = set_busy.clone();
        let push_log = push_log.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                // Small delay to allow WS to connect first
                gloo_timers::future::TimeoutFuture::new(200).await;
                set_busy.emit(true);
                match api::get_status().await {
                    Ok(s) => status.set(Some(StatusState {
                        usb_enabled: s.usb_enabled,
                        usb_ready: s.usb_ready,
                        host_os: s.host_os,
                    })),
                    Err(e) => push_log.emit(format!("status error: {e}")),
                }
                match api::get_config().await {
                    Ok(c) => config.set(Some(ConfigState {
                        usb_manufacturer: c.usb_manufacturer,
                        usb_product: c.usb_product,
                    })),
                    Err(e) => push_log.emit(format!("config error: {e}")),
                }
                match api::list_scripts().await {
                    Ok(list) => scripts_state.set(Some(list)),
                    Err(e) => push_log.emit(format!("scripts error: {e}")),
                }
                set_busy.emit(false);
            });
            || ()
        });
    }

    // Poll status every 5s
    {
        let status = status.clone();
        let push_log = push_log.clone();
        use_effect_with((), move |_| {
            let handle = gloo_timers::callback::Interval::new(5000, move || {
                let status = status.clone();
                let push_log = push_log.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    match api::get_status().await {
                        Ok(s) => status.set(Some(StatusState {
                            usb_enabled: s.usb_enabled,
                            usb_ready: s.usb_ready,
                            host_os: s.host_os,
                        })),
                        Err(e) => push_log.emit(format!("status error: {e}")),
                    }
                });
            });
            move || drop(handle)
        });
    }

    // Open a single WebSocket via api module and log messages
    {
        let push_log = push_log.clone();
        let ws_connected = ws_connected.clone();
        use_effect_with((), move |_| {
            api::init_ws(move |s| push_log.emit(s), move |b| ws_connected.set(b));
            || ()
        });
    }

    // Actions
    let on_save_identity = {
        let config = config.clone();
        let set_busy = set_busy.clone();
        let push_log = push_log.clone();
        let toast_cb = show_toast.clone();
        Callback::from(move |(man, prod): (String, String)| {
            let config = config.clone();
            let set_busy = set_busy.clone();
            let push_log = push_log.clone();
            let show_toast = toast_cb.clone();
            wasm_bindgen_futures::spawn_local(async move {
                // client-side validation similar to firmware
                if man.len() > 32 || prod.len() > 48 || !man.is_ascii() || !prod.is_ascii() {
                    push_log.emit("invalid identity (length/ascii)".to_string());
                    return;
                }
                set_busy.emit(true);
                match api::save_config(&man, &prod).await {
                    Ok(()) => {
                        push_log.emit("identity saved".to_string());
                        show_toast.emit(("Identity saved".into(), true));
                        config.set(Some(ConfigState {
                            usb_manufacturer: man,
                            usb_product: prod,
                        }));
                    }
                    Err(e) => {
                        push_log.emit(format!("save identity error: {e}"));
                        show_toast.emit((format!("Save failed: {e}"), false));
                    }
                }
                set_busy.emit(false);
            });
        })
    };

    let on_usb_start =
        {
            let selected_os = selected_os.clone();
            let set_busy = set_busy.clone();
            let push_log = push_log.clone();
            let toast_cb = show_toast.clone();
            Callback::from(move |assistant: bool| {
                let selected_os = (*selected_os).clone();
                let set_busy = set_busy.clone();
                let push_log = push_log.clone();
                let show_toast = toast_cb.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    set_busy.emit(true);
                    let os_opt = if selected_os == "unknown" {
                        None
                    } else {
                        Some(selected_os.as_str())
                    };
                    match api::usb_register(assistant, os_opt).await {
                        Ok(()) => {
                            push_log.emit("USB enabling request sent".to_string());
                            show_toast.emit(("USB enabling…".into(), true));
                            if assistant {
                                match scripts::lookup("assistant_once") {
                                    Some(script_dsl) => match dsl::compile(script_dsl) {
                                        Ok(bytecode) => match api::run_script(&bytecode).await {
                                            Ok(()) => push_log
                                                .emit("macOS assistant script queued".into()),
                                            Err(e) => push_log
                                                .emit(format!("assistant script failed: {e}")),
                                        },
                                        Err(err) => {
                                            push_log.emit(format!(
                                                "assistant compile error: {}",
                                                err.message
                                            ));
                                        }
                                    },
                                    None => push_log
                                        .emit("assistant script unavailable in frontend".into()),
                                }
                            }
                        }
                        Err(e) => {
                            push_log.emit(format!("usb start error: {e}"));
                            show_toast.emit((format!("USB start failed: {e}"), false));
                        }
                    }
                    set_busy.emit(false);
                });
            })
        };

    let on_run_dsl = {
        let dsl_text = dsl_text.clone();
        let set_busy = set_busy.clone();
        let push_log = push_log.clone();
        let toast_cb = show_toast.clone();
        Callback::from(move |_| {
            let txt = (*dsl_text).clone();
            if txt.trim().is_empty() {
                push_log.emit("empty script".to_string());
                return;
            }
            let set_busy = set_busy.clone();
            let push_log = push_log.clone();
            let show_toast = toast_cb.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let bytecode = match dsl::compile(&txt) {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        push_log.emit(format!("compile error: {}", err.message));
                        show_toast.emit((format!("Compile failed: {}", err.message), false));
                        return;
                    }
                };
                set_busy.emit(true);
                match api::run_script(&bytecode).await {
                    Ok(()) => {
                        push_log.emit(format!("queued ({} bytes)", bytecode.len()));
                        show_toast.emit(("Script queued".into(), true));
                    }
                    Err(e) => {
                        push_log.emit(format!("run error: {e}"));
                        show_toast.emit((format!("Run failed: {e}"), false));
                    }
                }
                set_busy.emit(false);
            });
        })
    };

    // UI pieces
    let st = (*status).clone();
    let conf = (*config).clone();
    let scr = (*scripts).clone();
    let busy = *busy_count > 0;

    html! {
        <div class="container">
            <ToastBar toast={(*toast).clone()} />
            <div class="hero">
                <div class="hero-text">
                    <h1>{"Pico Endpoint"}</h1>
                    <p class="sub">{"A compact control surface for bring-up and scripting."}</p>
                </div>
            </div>

            <div class="grid">
                <StatusCard status={st.clone()} busy={busy} ws_connected={*ws_connected} />
                <IdentityCard
                    config={conf.clone()}
                    usb_enabled={st.as_ref().map(|s| s.usb_enabled).unwrap_or(false)}
                    on_save={on_save_identity.clone()} />
                <UsbCard
                    selected_os={(*selected_os).clone()}
                    on_select_os={
                        let selected_os = selected_os.clone();
                        Callback::from(move |os: String| selected_os.set(os))
                    }
                    usb_enabled={st.as_ref().map(|s| s.usb_enabled).unwrap_or(false)}
                    on_start={on_usb_start.clone()}
                    on_stop={
                        let set_busy = set_busy.clone();
                        let push_log = push_log.clone();
                        let toast_cb = show_toast.clone();
                        Callback::from(move |_| {
                            let set_busy = set_busy.clone();
                            let push_log = push_log.clone();
                            let show_toast = toast_cb.clone();
                            wasm_bindgen_futures::spawn_local(async move {
                                set_busy.emit(true);
                                match api::usb_unregister().await {
                                    Ok(()) => { push_log.emit("USB disabling request sent".into()); show_toast.emit(("USB disabling…".into(), true)); }
                                    Err(e) => { push_log.emit(format!("usb stop error: {e}")); show_toast.emit((format!("USB stop failed: {e}"), false)); }
                                }
                                set_busy.emit(false);
                            });
                        })
                    }
                />
                <ScriptingCard
                    dsl_text={(*dsl_text).clone()}
                    on_change={
                        let dsl_text = dsl_text.clone();
                        Callback::from(move |s: String| dsl_text.set(s))
                    }
                    scripts={scr.clone()}
                    on_run={on_run_dsl.clone()} />
                <LogCard
                    lines={(*log_lines).clone()}
                    on_clear={
                        let log_lines = log_lines.clone();
                        Callback::from(move |_| log_lines.set(Vec::new()))
                    } />
            </div>
        </div>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct StatusProps {
    pub status: Option<StatusState>,
    pub busy: bool,
    pub ws_connected: bool,
}
#[function_component(StatusCard)]
fn status_card(props: &StatusProps) -> Html {
    let st = &props.status;
    let usb_enabled = st.as_ref().map(|s| s.usb_enabled).unwrap_or(false);
    let usb_ready = st.as_ref().map(|s| s.usb_ready).unwrap_or(false);
    let host_os = st
        .as_ref()
        .map(|s| s.host_os.clone())
        .unwrap_or_else(|| "unknown".into());
    html! {
        <div class="card" aria-busy={props.busy.to_string()}>
            <div class="row between items-center">
                <h2>{"Status"}</h2>
                <div id="spinner" class="hide-sm" style={format!("display:{}", if props.busy {"inline-flex"} else {"none"})}>{"Working…"}</div>
            </div>
            <div id="status" class="status-bar my-1">
                <span id="stUsbEnabled" class={classes!("badge", if usb_enabled {"on"} else {"off"})} title="USB device registration">{"🔌 USB: "}{ if usb_enabled {"on"} else {"off"} }</span>
                <span id="stUsbReady" class={classes!("badge", if usb_ready {"on"} else {"off"})} title="USB host ready">{"⌨️ Ready: "}{ if usb_ready {"yes"} else {"no"} }</span>
                <span id="stHostOs" class={classes!("badge", match host_os.as_str() {"mac"=>"mac","windows"=>"windows",_=>"neutral"})} title="Host OS">{"🖥️ OS: "}{host_os}</span>
                <span id="stWs" class={classes!("badge", if props.ws_connected {"on"} else {"off"})} title="WebSocket">{"🔗 WS: "}{ if props.ws_connected {"connected"} else {"disconnected"} }</span>
            </div>
            <div class="hint">{"Refreshes every 5s."}</div>
        </div>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct IdentityProps {
    pub config: Option<ConfigState>,
    pub usb_enabled: bool,
    pub on_save: Callback<(String, String)>,
}
#[function_component(IdentityCard)]
fn identity_card(props: &IdentityProps) -> Html {
    let man = use_state(|| {
        props
            .config
            .as_ref()
            .map(|c| c.usb_manufacturer.clone())
            .unwrap_or_default()
    });
    let prod = use_state(|| {
        props
            .config
            .as_ref()
            .map(|c| c.usb_product.clone())
            .unwrap_or_default()
    });

    // Sync local inputs when props update
    {
        let man = man.clone();
        let prod = prod.clone();
        let cfg = props.config.clone();
        use_effect_with(props.config.clone(), move |_| {
            if let Some(c) = cfg {
                man.set(c.usb_manufacturer);
                prod.set(c.usb_product);
            }
            || ()
        });
    }

    // Disable save when USB is enabled or nothing changed
    let orig_man = props
        .config
        .as_ref()
        .map(|c| c.usb_manufacturer.clone())
        .unwrap_or_default();
    let orig_prod = props
        .config
        .as_ref()
        .map(|c| c.usb_product.clone())
        .unwrap_or_default();
    let dirty = *man != orig_man || *prod != orig_prod;
    let fields_disabled = props.usb_enabled;
    let disabled = props.usb_enabled || !dirty; // for Save button only
    let on_save = {
        let man = man.clone();
        let prod = prod.clone();
        let cb = props.on_save.clone();
        Callback::from(move |_| cb.emit(((*man).clone(), (*prod).clone())))
    };
    html! {
        <div class="card" id="identityCard">
          <h2>{"Device Identity"}</h2>
          <div class="row gap-4 items-end wrap">
            <label class="field flex-1">
              <div>
                <span>{"USB manufacturer"}</span>
                <input id="usbManufacturer" type="text" placeholder="Pico 2W" maxlength="32"
                  value={(*man).clone()}
                  oninput={{ let man=man.clone(); Callback::from(move |e: InputEvent| man.set(e.target_unchecked_into::<web_sys::HtmlInputElement>().value())) }}
                  disabled={fields_disabled} />
                <div class="row between">
                  <span class="hint">{"ASCII only, max 32 bytes."}</span>
                  <span class="hint">{ format!("{}/32", (*man).as_bytes().len()) }</span>
                </div>
              </div>
            </label>
            <label class="field flex-2">
              <div>
                <span>{"USB product string"}</span>
                <input id="usbProduct" type="text" placeholder="Logger + Keyboard" maxlength="48"
                  value={(*prod).clone()}
                  oninput={{ let prod=prod.clone(); Callback::from(move |e: InputEvent| prod.set(e.target_unchecked_into::<web_sys::HtmlInputElement>().value())) }}
                  disabled={fields_disabled} />
                <div class="row between">
                  <span class="hint">{"ASCII only, max 48 bytes."}</span>
                  <span class="hint">{ format!("{}/48", (*prod).as_bytes().len()) }</span>
                </div>
              </div>
            </label>
            <button id="btnSaveIdentity" class="btn-accent" {disabled} onclick={on_save}>{"Save"}</button>
          </div>
          <div class="hint">{"Applies when you start USB. Changes are blocked after USB is enabled."}</div>
        </div>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct UsbProps {
    pub selected_os: String,
    pub on_select_os: Callback<String>,
    pub on_start: Callback<bool>,
    pub on_stop: Callback<()>,
    pub usb_enabled: bool,
}
#[function_component(UsbCard)]
fn usb_card(props: &UsbProps) -> Html {
    let on_select = props.on_select_os.clone();
    let set_mac = Callback::from(move |_| on_select.emit("mac".to_string()));
    let on_select = props.on_select_os.clone();
    let set_win = Callback::from(move |_| on_select.emit("windows".to_string()));
    let start_assist = {
        let cb = props.on_start.clone();
        Callback::from(move |_| cb.emit(true))
    };
    let start_noassist = {
        let cb = props.on_start.clone();
        Callback::from(move |_| cb.emit(false))
    };
    html! {
        <div class="card" id="usbCard">
          <h2>{"USB Bring-up"}</h2>
          if !props.usb_enabled {
            <>
              <div class="row radio-bar mb-1">
                <label class="radio"><input type="radio" name="os" value="mac" checked={props.selected_os=="mac"} onclick={set_mac}/>{" macOS"}</label>
                <label class="radio"><input type="radio" name="os" value="windows" checked={props.selected_os=="windows"} onclick={set_win}/>{" Windows"}</label>
              </div>
              <div class="row gap-3">
                <button id="btnUsbAssistant" class="btn-accent" onclick={start_assist} disabled={props.selected_os=="windows"}>{"Start USB on macOS (Assistant)"}</button>
                <button id="btnUsbNoAssistant" class="btn-secondary" onclick={start_noassist}>{"Start USB"}</button>
              </div>
              <div class="hint">{"Assistant forces macOS Keyboard Setup Assistant sequence when enabled."}</div>
            </>
          } else {
            <div class="row gap-3">
              <button id="btnUsbStop" class="btn-danger" onclick={{ let cb = props.on_stop.clone(); Callback::from(move |_| cb.emit(())) }}>{"Stop USB"}</button>
            </div>
            <div class="hint">{"Stops the composite USB device until you start it again."}</div>
          }
        </div>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct ScriptProps {
    pub dsl_text: String,
    pub on_change: Callback<String>,
    pub scripts: Option<Vec<api::ScriptMeta>>,
    pub on_run: Callback<()>,
}
#[function_component(ScriptingCard)]
fn scripting_card(props: &ScriptProps) -> Html {
    let on_text = {
        let cb = props.on_change.clone();
        Callback::from(move |e: InputEvent| {
            cb.emit(
                e.target_unchecked_into::<web_sys::HtmlTextAreaElement>()
                    .value(),
            )
        })
    };

    let on_keydown = {
        let cb = props.on_run.clone();
        Callback::from(move |e: KeyboardEvent| {
            if (e.ctrl_key() || e.meta_key()) && e.key() == "Enter" {
                e.prevent_default();
                cb.emit(());
            }
        })
    };

    let insert_script = |text: String| {
        let cb = props.on_change.clone();
        move |_| {
            cb.emit(text.clone());
        }
    };

    html! {
      <div class="card full">
        <h2>{"Scripting"}</h2>
        <div class="row column gap-2 mb-1">
          <textarea id="scriptDsl" rows="6" cols="60" placeholder={"tap(\"ENTER\")\nmodtap(\"LGUI+SPACE\")\ndelay(400)\ntext(\"Terminal\", 10)"}
            value={props.dsl_text.clone()} oninput={on_text} onkeydown={on_keydown} />
          <div class="row">
            <button id="btnRunDsl" class="btn-accent" onclick={{ let cb=props.on_run.clone(); Callback::from(move |_| cb.emit(())) }}>{"Run Script"}</button>
            <span class="hint">{"Commands: "}<code>{"tap(\"KEY\")"}</code>{"; "}<code>{"modtap(\"MOD+KEY\")"}</code>{"; "}<code>{"delay(MS)"}</code>{"; "}<code>{"text(\"STRING\", [DELAY])"}</code>{" — Press Ctrl/⌘+Enter to run."}</span>
            {{
              let lines = props.dsl_text.lines().count();
              let style = if lines > MAX_DSL_LINES { "color: var(--bad)".to_string() } else { String::new() };
              html! { <span class="hint" style={style}>{ format!("{} lines (max {})", lines, MAX_DSL_LINES) }</span> }
            }}
          </div>
        </div>
        if let Some(list) = &props.scripts {
          <div class="row column gap-2">
            <div class="hint">{"Built-in scripts:"}</div>
            { for list.iter().map(|s| html!{
                <div class="row between gap-2">
                  <div><strong>{&s.name}</strong>{": "}{&s.description}</div>
                  <div class="row">
                    if let Some(pre) = &s.dsl {
                      <button onclick={insert_script(pre.clone())}>{"Use"}</button>
                    }
                  </div>
                </div>
              }) }
            </div>
          }
        </div>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct LogProps {
    pub lines: Vec<String>,
    pub on_clear: Callback<()>,
}
#[function_component(LogCard)]
fn log_card(props: &LogProps) -> Html {
    let node_ref = use_node_ref();
    {
        let node_ref = node_ref.clone();
        let len = props.lines.len();
        use_effect_with(len, move |_| {
            if let Some(el) = node_ref.cast::<web_sys::HtmlElement>() {
                el.set_scroll_top(el.scroll_height());
            }
            || ()
        });
    }
    html! {
        <div class="card full">
          <div class="row between items-center">
            <h2 class="m-0">{"Log"}</h2>
            <button id="btnClearLog" class="btn-secondary" onclick={{ let cb=props.on_clear.clone(); Callback::from(move |_| cb.emit(())) }}>{"Clear Log"}</button>
          </div>
          <div id="log" ref={node_ref}>{ for props.lines.iter().map(|l| html!{ <div>{l}</div> }) }</div>
        </div>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct ToastProps {
    pub toast: Option<(String, bool)>,
}
#[function_component(ToastBar)]
fn toast_bar(props: &ToastProps) -> Html {
    if let Some((msg, ok)) = &props.toast {
        let class = if *ok { "toast ok" } else { "toast err" };
        let role = if *ok { "status" } else { "alert" };
        html! {
          <div role={role} aria-live="polite" class={class.to_string()}>
            { msg }
          </div>
        }
    } else {
        html! {}
    }
}

#[wasm_bindgen(start)]
pub fn main_js() {
    yew::Renderer::<App>::new().render();
}
