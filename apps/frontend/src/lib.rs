use wasm_bindgen::prelude::*;
use yew::prelude::*;

use dsl_core::MAX_DSL_LINES;

mod api;
pub mod codec;
pub mod dsl;
mod filesystem;
pub mod scripts;
mod transfer;

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

#[derive(Clone)]
struct BrowseRequest {
    path: String,
    cursor: u32,
    append: bool,
    show_hidden: bool,
}

#[function_component(App)]
fn app() -> Html {
    let status: UseStateHandle<Option<StatusState>> = use_state(|| None);
    let config: UseStateHandle<Option<ConfigState>> = use_state(|| None);
    let scripts: UseStateHandle<Option<Vec<api::ScriptMeta>>> = use_state(|| None);
    let transfer_store = use_mut_ref(transfer::TransferStore::new);
    let transfers = use_state(Vec::<transfer::TransferView>::new);
    let transfer_path = use_state(String::new);
    let filesystem_store = use_mut_ref(filesystem::BrowserStore::new);
    let filesystem_view = use_state(filesystem::BrowserView::default);
    let dsl_text = use_state(String::new);
    let selected_os = use_state(|| String::from("mac"));
    let selected_layout = use_state(|| dsl_core::DEFAULT_LAYOUT_ID.to_string());
    let busy_count = use_state(|| 0u32);
    let log_lines = use_state(Vec::<String>::new);
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

    let on_browse = {
        let filesystem_store = filesystem_store.clone();
        let filesystem_view = filesystem_view.clone();
        let push_log = push_log.clone();
        Callback::from(move |request: BrowseRequest| {
            let request_id = api::next_filesystem_request_id();
            let previous_request_id = {
                let mut store = filesystem_store.borrow_mut();
                let previous = store.begin_request(request_id, request.append, request.show_hidden);
                filesystem_view.set(store.snapshot());
                previous
            };

            let filesystem_store = filesystem_store.clone();
            let filesystem_view = filesystem_view.clone();
            let push_log = push_log.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Some(previous_request_id) = previous_request_id {
                    let _ = api::filesystem_cancel(previous_request_id).await;
                }
                if let Err(error) = api::filesystem_list(
                    request_id,
                    &request.path,
                    request.cursor,
                    64,
                    request.show_hidden,
                )
                .await
                {
                    let mut store = filesystem_store.borrow_mut();
                    store.fail_request(request_id, error.clone());
                    filesystem_view.set(store.snapshot());
                    push_log.emit(format!("filesystem list error: {error}"));
                    return;
                }

                gloo_timers::future::TimeoutFuture::new(15_000).await;
                let mut store = filesystem_store.borrow_mut();
                store.fail_request(
                    request_id,
                    "filesystem request timed out; ensure the host-agent is running".to_string(),
                );
                filesystem_view.set(store.snapshot());
            });
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
        let transfer_store = transfer_store.clone();
        let transfers_state = transfers.clone();
        let filesystem_store = filesystem_store.clone();
        let filesystem_view = filesystem_view.clone();
        use_effect_with((), move |_| {
            api::init_ws(
                {
                    let push_log = push_log.clone();
                    move |s| push_log.emit(s)
                },
                {
                    let ws_connected = ws_connected.clone();
                    move |b| ws_connected.set(b)
                },
                {
                    let push_log = push_log.clone();
                    let transfer_store = transfer_store.clone();
                    let transfers_state = transfers_state.clone();
                    move |event_json| {
                        let now_ms = monotonic_now_ms();
                        let mut store = transfer_store.borrow_mut();
                        if let Err(err) = store.apply_text_event(&event_json, now_ms) {
                            push_log.emit(format!("transfer event error: {err}"));
                            return;
                        }
                        transfers_state.set(store.snapshots());
                    }
                },
                {
                    let push_log = push_log.clone();
                    let transfer_store = transfer_store.clone();
                    let transfers_state = transfers_state.clone();
                    move |binary| {
                        let now_ms = monotonic_now_ms();
                        let mut store = transfer_store.borrow_mut();
                        if let Err(err) = store.apply_binary_chunk(&binary, now_ms) {
                            push_log.emit(format!("transfer chunk error: {err}"));
                            return;
                        }
                        transfers_state.set(store.snapshots());
                    }
                },
                {
                    let push_log = push_log.clone();
                    let filesystem_store = filesystem_store.clone();
                    let filesystem_view = filesystem_view.clone();
                    move |binary| match filesystem::decode_list_page(&binary) {
                        Ok(page) => {
                            let mut store = filesystem_store.borrow_mut();
                            if store.apply_page(page) {
                                filesystem_view.set(store.snapshot());
                            }
                        }
                        Err(error) => {
                            push_log.emit(format!("filesystem page error: {error}"));
                        }
                    }
                },
            );
            || ()
        });
    }

    {
        let on_browse = on_browse.clone();
        use_effect_with(*ws_connected, move |connected| {
            if *connected {
                on_browse.emit(BrowseRequest {
                    path: String::new(),
                    cursor: 0,
                    append: false,
                    show_hidden: false,
                });
            }
            || ()
        });
    }

    // Refresh ETA/rate view at a stable cadence to avoid render thrash.
    {
        let transfer_store = transfer_store.clone();
        let transfers_state = transfers.clone();
        use_effect_with((), move |_| {
            let interval = gloo_timers::callback::Interval::new(300, move || {
                let now_ms = monotonic_now_ms();
                let mut store = transfer_store.borrow_mut();
                store.tick(now_ms);
                transfers_state.set(store.snapshots());
            });
            move || drop(interval)
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

    let on_usb_start = {
        let selected_os = selected_os.clone();
        let selected_layout = selected_layout.clone();
        let set_busy = set_busy.clone();
        let push_log = push_log.clone();
        let toast_cb = show_toast.clone();
        Callback::from(move |assistant: bool| {
            let selected_os = (*selected_os).clone();
            let layout_id = (*selected_layout).clone();
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
                            match scripts::lookup("assistant_us") {
                                Some(script_dsl) => {
                                    match dsl::compile(script_dsl, &layout_id) {
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
                                    }
                                }
                                None => {
                                    push_log.emit("assistant script unavailable in frontend".into())
                                }
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
        let selected_layout = selected_layout.clone();
        let set_busy = set_busy.clone();
        let push_log = push_log.clone();
        let toast_cb = show_toast.clone();
        Callback::from(move |_| {
            let txt = (*dsl_text).clone();
            let layout_id = (*selected_layout).clone();
            if txt.trim().is_empty() {
                push_log.emit("empty script".to_string());
                return;
            }
            let set_busy = set_busy.clone();
            let push_log = push_log.clone();
            let show_toast = toast_cb.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let bytecode = match dsl::compile(&txt, &layout_id) {
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

    let on_download_transfer = {
        let transfer_store = transfer_store.clone();
        let transfers_state = transfers.clone();
        let push_log = push_log.clone();
        Callback::from(move |transfer_id: u64| {
            let plan = {
                let mut store = transfer_store.borrow_mut();
                match store.prepare_finalize(transfer_id) {
                    Ok(plan) => {
                        transfers_state.set(store.snapshots());
                        plan
                    }
                    Err(err) => {
                        push_log.emit(format!("finalize failed: {err}"));
                        return;
                    }
                }
            };

            let transfer_store = transfer_store.clone();
            let transfers_state = transfers_state.clone();
            let push_log = push_log.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = transfer::finalize_and_download(&plan).await;
                {
                    let mut store = transfer_store.borrow_mut();
                    store.finish_finalize(transfer_id, &result);
                    transfers_state.set(store.snapshots());
                }
                match result {
                    Ok(()) => push_log.emit(format!("download ready: {}", plan.file_name)),
                    Err(err) => push_log.emit(format!("download failed: {err}")),
                }
            });
        })
    };

    let queue_transfer_path = {
        let set_busy = set_busy.clone();
        let push_log = push_log.clone();
        let toast_cb = show_toast.clone();
        Callback::from(move |path: String| {
            let path = path.trim().to_string();
            if path.is_empty() {
                push_log.emit("transfer path is empty".into());
                return;
            }
            let set_busy = set_busy.clone();
            let push_log = push_log.clone();
            let show_toast = toast_cb.clone();
            wasm_bindgen_futures::spawn_local(async move {
                set_busy.emit(true);
                match api::transfer_start(&path).await {
                    Ok(()) => {
                        push_log.emit(format!("transfer queued for path: {}", path));
                        show_toast.emit(("Transfer queued".into(), true));
                    }
                    Err(err) => {
                        push_log.emit(format!("transfer start error: {err}"));
                        show_toast.emit((format!("Transfer start failed: {err}"), false));
                    }
                }
                set_busy.emit(false);
            });
        })
    };

    let on_start_transfer = {
        let transfer_path = transfer_path.clone();
        let queue_transfer_path = queue_transfer_path.clone();
        Callback::from(move |_| queue_transfer_path.emit((*transfer_path).clone()))
    };

    let on_set_transfer_default = {
        let transfer_path = transfer_path.clone();
        let set_busy = set_busy.clone();
        let push_log = push_log.clone();
        let toast_cb = show_toast.clone();
        Callback::from(move |_| {
            let path = (*transfer_path).trim().to_string();
            if path.is_empty() {
                push_log.emit("transfer path is empty".into());
                return;
            }
            let set_busy = set_busy.clone();
            let push_log = push_log.clone();
            let show_toast = toast_cb.clone();
            wasm_bindgen_futures::spawn_local(async move {
                set_busy.emit(true);
                match api::transfer_set_default(&path).await {
                    Ok(()) => {
                        push_log.emit(format!("default transfer path set: {}", path));
                        show_toast.emit(("Transfer default updated".into(), true));
                    }
                    Err(err) => {
                        push_log.emit(format!("transfer default error: {err}"));
                        show_toast.emit((format!("Set default failed: {err}"), false));
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
                    selected_layout={(*selected_layout).clone()}
                    on_select_layout={
                        let selected_layout = selected_layout.clone();
                        Callback::from(move |layout: String| selected_layout.set(layout))
                    }
                    scripts={scr.clone()}
                    on_run={on_run_dsl.clone()} />
                <DownloadManagerCard
                    transfers={(*transfers).clone()}
                    on_download={on_download_transfer.clone()} />
                <FileBrowserCard
                    view={(*filesystem_view).clone()}
                    connected={*ws_connected}
                    on_browse={on_browse.clone()}
                    on_select={{
                        let transfer_path = transfer_path.clone();
                        Callback::from(move |path: String| transfer_path.set(path))
                    }}
                    on_transfer={queue_transfer_path.clone()} />
                <TransferStartCard
                    path={(*transfer_path).clone()}
                    on_change={
                        let transfer_path = transfer_path.clone();
                        Callback::from(move |value: String| transfer_path.set(value))
                    }
                    on_start={on_start_transfer.clone()}
                    on_set_default={on_set_transfer_default.clone()} />
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
                  <span class="hint">{ format!("{}/32", (*man).len()) }</span>
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
                  <span class="hint">{ format!("{}/48", (*prod).len()) }</span>
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
    pub selected_layout: String,
    pub on_select_layout: Callback<String>,
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

    let on_layout = {
        let cb = props.on_select_layout.clone();
        Callback::from(move |e: Event| {
            cb.emit(
                e.target_unchecked_into::<web_sys::HtmlSelectElement>()
                    .value(),
            );
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

    let layouts = dsl_core::available_layouts();

    html! {
      <div class="card full">
        <h2>{"Scripting"}</h2>
        <div class="row column gap-2 mb-1">
          <textarea id="scriptDsl" rows="6" cols="60" placeholder={"tap(\"ENTER\")\nmodtap(\"LGUI+SPACE\")\ndelay(400)\ntext(\"Terminal\", 10)"}
            value={props.dsl_text.clone()} oninput={on_text} onkeydown={on_keydown} />
          <div class="row gap-2 items-center">
            <label class="hint" for="scriptLayout">{"Layout"}</label>
            <select id="scriptLayout" value={props.selected_layout.clone()} onchange={on_layout}>
              { for layouts.iter().map(|id| html!{ <option value={id.to_string()}>{ *id }</option> }) }
            </select>
            <span class="hint">{"Default for text(); layout(\"...\") overrides."}</span>
          </div>
          <div class="row">
            <button id="btnRunDsl" class="btn-accent" onclick={{ let cb=props.on_run.clone(); Callback::from(move |_| cb.emit(())) }}>{"Run Script"}</button>
            <span class="hint">{"Commands: "}<code>{"tap(\"KEY\")"}</code>{"; "}<code>{"modtap(\"MOD+KEY\")"}</code>{"; "}<code>{"delay(MS)"}</code>{"; "}<code>{"text(\"STRING\", [DELAY])"}</code>{"; "}<code>{"layout(\"ID\")"}</code>{" — Press Ctrl/⌘+Enter to run."}</span>
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
struct FileBrowserProps {
    pub view: filesystem::BrowserView,
    pub connected: bool,
    pub on_browse: Callback<BrowseRequest>,
    pub on_select: Callback<String>,
    pub on_transfer: Callback<String>,
}

#[function_component(FileBrowserCard)]
fn file_browser_card(props: &FileBrowserProps) -> Html {
    let browse_home = {
        let on_browse = props.on_browse.clone();
        let show_hidden = props.view.show_hidden;
        Callback::from(move |_| {
            on_browse.emit(BrowseRequest {
                path: String::new(),
                cursor: 0,
                append: false,
                show_hidden,
            })
        })
    };
    let browse_root = {
        let on_browse = props.on_browse.clone();
        let show_hidden = props.view.show_hidden;
        Callback::from(move |_| {
            on_browse.emit(BrowseRequest {
                path: "/".to_string(),
                cursor: 0,
                append: false,
                show_hidden,
            })
        })
    };
    let browse_parent = {
        let on_browse = props.on_browse.clone();
        let show_hidden = props.view.show_hidden;
        let parent = filesystem::parent_path(&props.view.directory);
        Callback::from(move |_| {
            on_browse.emit(BrowseRequest {
                path: parent.clone(),
                cursor: 0,
                append: false,
                show_hidden,
            })
        })
    };
    let refresh = {
        let on_browse = props.on_browse.clone();
        let show_hidden = props.view.show_hidden;
        let path = props.view.directory.clone();
        Callback::from(move |_| {
            on_browse.emit(BrowseRequest {
                path: path.clone(),
                cursor: 0,
                append: false,
                show_hidden,
            })
        })
    };
    let toggle_hidden = {
        let on_browse = props.on_browse.clone();
        let path = props.view.directory.clone();
        Callback::from(move |event: Event| {
            let show_hidden = event
                .target_unchecked_into::<web_sys::HtmlInputElement>()
                .checked();
            on_browse.emit(BrowseRequest {
                path: path.clone(),
                cursor: 0,
                append: false,
                show_hidden,
            });
        })
    };
    let load_more = {
        let on_browse = props.on_browse.clone();
        let path = props.view.directory.clone();
        let cursor = props.view.next_cursor;
        let show_hidden = props.view.show_hidden;
        Callback::from(move |_| {
            on_browse.emit(BrowseRequest {
                path: path.clone(),
                cursor,
                append: true,
                show_hidden,
            })
        })
    };

    html! {
        <div class="card full file-browser">
          <div class="row between items-center">
            <h2 class="m-0">{"Source Files"}</h2>
            <span class="hint">
              { if props.connected { "Host filesystem via USB agent" } else { "WebSocket disconnected" } }
            </span>
          </div>

          <div class="row gap-2 my-1">
            <button onclick={browse_home} disabled={!props.connected || props.view.loading}>{"Home"}</button>
            <button onclick={browse_root} disabled={!props.connected || props.view.loading}>{"/"}</button>
            <button
                onclick={browse_parent}
                disabled={!props.connected || props.view.loading || props.view.directory.is_empty() || props.view.directory == "/"}
            >
              {"Up"}
            </button>
            <button onclick={refresh} disabled={!props.connected || props.view.loading}>{"Refresh"}</button>
            <label class="inline fs-hidden">
              <input type="checkbox" checked={props.view.show_hidden} onchange={toggle_hidden} />
              {"Show hidden"}
            </label>
          </div>

          <nav class="fs-breadcrumbs" aria-label="Current directory">
            if props.view.directory.is_empty() {
              <span class="hint">{"Waiting for host-agent…"}</span>
            } else {
              { for filesystem::breadcrumbs(&props.view.directory).into_iter().map(|(label, path)| {
                  let on_browse = props.on_browse.clone();
                  let show_hidden = props.view.show_hidden;
                  let onclick = Callback::from(move |_| {
                      on_browse.emit(BrowseRequest {
                          path: path.clone(),
                          cursor: 0,
                          append: false,
                          show_hidden,
                      });
                  });
                  html! {
                    <button class="fs-crumb" {onclick} disabled={props.view.loading}>{label}</button>
                  }
              }) }
            }
          </nav>

          if let Some(error) = &props.view.error {
            <div class="fs-error">{error}</div>
          }

          <div class="fs-table" role="table" aria-label="Source directory">
            <div class="fs-row fs-header" role="row">
              <span>{"Name"}</span>
              <span>{"Size"}</span>
              <span>{"Modified"}</span>
              <span>{"Actions"}</span>
            </div>
            if props.view.entries.is_empty() && !props.view.loading && props.view.error.is_none() {
              <div class="hint">{"This directory is empty."}</div>
            }
            { for props.view.entries.iter().map(|entry| {
                let full_path = filesystem::join_path(&props.view.directory, &entry.name);
                let icon = match entry.kind {
                    filesystem::EntryKind::Directory => "📁",
                    filesystem::EntryKind::SymlinkDirectory => "🔗📁",
                    filesystem::EntryKind::File => "📄",
                    filesystem::EntryKind::SymlinkFile => "🔗📄",
                    filesystem::EntryKind::Other => "•",
                };
                let name_action = if entry.kind.is_directory() {
                    let on_browse = props.on_browse.clone();
                    let path = full_path.clone();
                    let show_hidden = props.view.show_hidden;
                    Callback::from(move |_| {
                        on_browse.emit(BrowseRequest {
                            path: path.clone(),
                            cursor: 0,
                            append: false,
                            show_hidden,
                        });
                    })
                } else {
                    let on_select = props.on_select.clone();
                    let path = full_path.clone();
                    Callback::from(move |_| on_select.emit(path.clone()))
                };
                let select = {
                    let on_select = props.on_select.clone();
                    let path = full_path.clone();
                    Callback::from(move |_| on_select.emit(path.clone()))
                };
                let transfer = {
                    let on_transfer = props.on_transfer.clone();
                    let path = full_path.clone();
                    Callback::from(move |_| on_transfer.emit(path.clone()))
                };
                html! {
                  <div class="fs-row" role="row">
                    <button
                        class="fs-name"
                        onclick={name_action}
                        disabled={!entry.readable || (!entry.kind.is_directory() && !entry.kind.is_file())}
                        title={full_path.clone()}
                    >
                      <span>{icon}</span>
                      <span>{&entry.name}</span>
                    </button>
                    <span class="fs-meta">
                      { if entry.kind.is_file() { format_bytes(entry.size) } else { "—".to_string() } }
                    </span>
                    <span class="fs-meta">{format_modified(entry.modified_secs)}</span>
                    <span class="row gap-1">
                      if entry.kind.is_file() {
                        <button class="btn-secondary" onclick={select} disabled={!entry.readable}>{"Select"}</button>
                        <button class="btn-accent" onclick={transfer} disabled={!entry.readable}>{"Transfer"}</button>
                      }
                    </span>
                  </div>
                }
            }) }
          </div>

          if props.view.loading {
            <div class="hint">{"Loading directory…"}</div>
          } else if props.view.has_more {
            <button class="btn-secondary" onclick={load_more}>{"Load more"}</button>
          }
        </div>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct TransferStartProps {
    pub path: String,
    pub on_change: Callback<String>,
    pub on_start: Callback<()>,
    pub on_set_default: Callback<()>,
}

#[function_component(TransferStartCard)]
fn transfer_start_card(props: &TransferStartProps) -> Html {
    let on_input = {
        let on_change = props.on_change.clone();
        Callback::from(move |event: InputEvent| {
            let value = event
                .target_unchecked_into::<web_sys::HtmlInputElement>()
                .value();
            on_change.emit(value);
        })
    };

    let on_start = {
        let cb = props.on_start.clone();
        Callback::from(move |_| cb.emit(()))
    };

    let on_set_default = {
        let cb = props.on_set_default.clone();
        Callback::from(move |_| cb.emit(()))
    };

    html! {
        <div class="card full">
          <h2>{"Start Transfer"}</h2>
          <div class="row gap-2">
            <input
                id="transferPath"
                type="text"
                placeholder="/Users/alice/Downloads/archive.zip"
                value={props.path.clone()}
                oninput={on_input}
            />
            <button class="btn-accent" onclick={on_start}>{"Queue Transfer"}</button>
            <button onclick={on_set_default}>{"Set Default"}</button>
          </div>
          <div class="hint">{"Path is resolved on the host-agent machine. Set Default updates the running host-agent without relaunching it."}</div>
        </div>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct DownloadManagerProps {
    pub transfers: Vec<transfer::TransferView>,
    pub on_download: Callback<u64>,
}

#[function_component(DownloadManagerCard)]
fn download_manager_card(props: &DownloadManagerProps) -> Html {
    html! {
        <div class="card full">
          <div class="row between items-center">
            <h2 class="m-0">{"Download Manager"}</h2>
            <span class="hint">{ format!("{} transfer(s)", props.transfers.len()) }</span>
          </div>
          if props.transfers.is_empty() {
            <div class="hint">{"Waiting for host-agent transfer events."}</div>
          } else {
            <div class="row column gap-2">
              { for props.transfers.iter().map(|transfer| {
                  let progress_pct = if transfer.total_size == 0 {
                      0.0
                  } else {
                      (transfer.received_size as f64 / transfer.total_size as f64 * 100.0).clamp(0.0, 100.0)
                  };
                  let on_download = {
                      let cb = props.on_download.clone();
                      let transfer_id = transfer.transfer_id;
                      Callback::from(move |_| cb.emit(transfer_id))
                  };
                  html! {
                    <div class="card transfer-row">
                      <div class="row between items-center">
                        <div>
                          <strong>{ &transfer.file_name }</strong>
                          <div class="hint">{ format!("id={} • {}", transfer.transfer_id, transfer_status_label(&transfer.status)) }</div>
                        </div>
                        <div class="row gap-2 items-center">
                          <span class="hint">{ format!("{} / {}", format_bytes(transfer.received_size), format_bytes(transfer.total_size)) }</span>
                          <span class="hint">{ format_rate(transfer.smoothed_rate_bps) }</span>
                          <span class="hint">{ format_eta(transfer.eta_total_secs) }</span>
                          <button
                            class="btn-secondary"
                            disabled={transfer.status != transfer::TransferState::Finished}
                            onclick={on_download}
                          >
                            {"Download"}
                          </button>
                        </div>
                      </div>
                      <div class="hint">{ format!(
                        "{:.1}% • chunks: total={} finished={} failed={} retrying={}",
                        progress_pct,
                        transfer.chunk_count,
                        transfer.finished_chunks,
                        transfer.failed_chunks,
                        transfer.counters.retrying
                      ) }</div>
                    </div>
                  }
              }) }
            </div>
          }
        </div>
    }
}

fn transfer_status_label(status: &transfer::TransferState) -> &'static str {
    match status {
        transfer::TransferState::Open => "open",
        transfer::TransferState::Downloading => "downloading",
        transfer::TransferState::Verifying => "verifying",
        transfer::TransferState::Finished => "finished",
        transfer::TransferState::Failed => "failed",
        transfer::TransferState::Aborted => "aborted",
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let value = bytes as f64;
    if value >= GB {
        format!("{:.2} GiB", value / GB)
    } else if value >= MB {
        format!("{:.2} MiB", value / MB)
    } else if value >= KB {
        format!("{:.1} KiB", value / KB)
    } else {
        format!("{} B", bytes)
    }
}

fn format_modified(modified_secs: Option<u64>) -> String {
    let Some(modified_secs) = modified_secs else {
        return "—".to_string();
    };

    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&JsValue::from_f64(modified_secs as f64 * 1000.0));
        date.to_locale_string("en-GB", &JsValue::UNDEFINED)
            .as_string()
            .unwrap_or_else(|| modified_secs.to_string())
    }

    #[cfg(not(target_arch = "wasm32"))]
    modified_secs.to_string()
}

fn format_rate(rate_bps: f64) -> String {
    if rate_bps <= 0.0 {
        return "rate: --".to_string();
    }
    format!("rate: {}/s", format_bytes(rate_bps as u64))
}

fn format_eta(eta_secs: Option<f64>) -> String {
    let Some(eta) = eta_secs else {
        return "ETA: unknown".to_string();
    };
    if !eta.is_finite() {
        return "ETA: unknown".to_string();
    }
    let eta = eta.max(0.0).round() as u64;
    let minutes = eta / 60;
    let seconds = eta % 60;
    if minutes > 0 {
        format!("ETA: {}m {:02}s", minutes, seconds)
    } else {
        format!("ETA: {}s", seconds)
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

fn monotonic_now_ms() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map(|performance| performance.now())
        .unwrap_or(0.0)
}

#[wasm_bindgen(start)]
pub fn main_js() {
    yew::Renderer::<App>::new().render();
}
