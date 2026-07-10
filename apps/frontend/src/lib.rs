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

const STARTER_SCRIPT: &str = "layout(\"mac_de-DE\")\nmodtap(\"LGUI+SPACE\")\ndelay(400)\ntext(\"Terminal\", 10)\ntap(\"ENTER\")";

#[derive(Clone, Copy, PartialEq)]
enum AppSection {
    Overview,
    Scripts,
    Transfers,
    Diagnostics,
}

#[derive(Clone, Copy, PartialEq)]
enum ConnectionState {
    Connecting,
    Connected,
    Reconnecting,
    Unavailable,
}

impl ConnectionState {
    fn label(self) -> &'static str {
        match self {
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Reconnecting => "Reconnecting",
            Self::Unavailable => "Unavailable",
        }
    }

    fn class(self) -> &'static str {
        match self {
            Self::Connected => "is-good",
            Self::Connecting | Self::Reconnecting => "is-warn",
            Self::Unavailable => "is-bad",
        }
    }
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
    let dsl_text = use_state(|| STARTER_SCRIPT.to_string());
    let selected_os = use_state(|| String::from("mac"));
    let active_section = use_state(|| AppSection::Overview);
    let busy_count = use_mut_ref(|| 0u32);
    let busy_state = use_state(|| false);
    let log_lines = use_state(Vec::<String>::new);
    let ws_connected = use_state(|| false);
    let connection_state = use_state(|| ConnectionState::Connecting);
    let was_connected = use_mut_ref(|| false);
    let toast = use_state(|| None::<(String, bool)>); // (message, ok?)
    // All WebSocket API calls are handled in api.rs via a single connection

    let set_busy = {
        let busy_count = busy_count.clone();
        let busy_state = busy_state.clone();
        Callback::from(move |on: bool| {
            let mut count = busy_count.borrow_mut();
            *count = if on {
                count.saturating_add(1)
            } else {
                count.saturating_sub(1)
            };
            busy_state.set(*count > 0);
        })
    };

    let push_log = {
        let log_lines = log_lines.clone();
        Callback::from(move |line: String| {
            let mut v = (*log_lines).clone();
            if v.last() == Some(&line) {
                return;
            }
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

    // Built-in scripts are compiled into the frontend and do not depend on the device.
    {
        let scripts_state = scripts.clone();
        let push_log = push_log.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match api::list_scripts().await {
                    Ok(list) => scripts_state.set(Some(list)),
                    Err(e) => push_log.emit(format!("scripts error: {e}")),
                }
            });
            || ()
        });
    }

    // Refresh device data whenever the socket opens or reconnects.
    {
        let status = status.clone();
        let config = config.clone();
        let set_busy = set_busy.clone();
        let push_log = push_log.clone();
        use_effect_with(*ws_connected, move |connected| {
            if *connected {
                let status = status.clone();
                let config = config.clone();
                let set_busy = set_busy.clone();
                let push_log = push_log.clone();
                wasm_bindgen_futures::spawn_local(async move {
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
                    set_busy.emit(false);
                });
            }
            || ()
        });
    }

    // Poll status only while connected; reconnecting does its own refresh.
    {
        let status = status.clone();
        let push_log = push_log.clone();
        use_effect_with(*ws_connected, move |connected| {
            let handle = connected.then(|| {
                gloo_timers::callback::Interval::new(5000, move || {
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
                })
            });
            move || drop(handle)
        });
    }

    // Open a single WebSocket via api module and log messages
    {
        let push_log = push_log.clone();
        let ws_connected = ws_connected.clone();
        let connection_state = connection_state.clone();
        let was_connected = was_connected.clone();
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
                    let connection_state = connection_state.clone();
                    let was_connected = was_connected.clone();
                    move |connected| {
                        ws_connected.set(connected);
                        if connected {
                            *was_connected.borrow_mut() = true;
                            connection_state.set(ConnectionState::Connected);
                        } else if *was_connected.borrow() {
                            connection_state.set(ConnectionState::Reconnecting);
                        } else {
                            connection_state.set(ConnectionState::Unavailable);
                        }
                    }
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
                    show_toast.emit((
                        "Identity must use ASCII and fit the byte limits".into(),
                        false,
                    ));
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
                                match scripts::lookup("assistant_us") {
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
                toast_cb.emit(("Add commands before running the script".into(), false));
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

    let on_download_transfer = {
        let transfer_store = transfer_store.clone();
        let transfers_state = transfers.clone();
        let push_log = push_log.clone();
        let toast_cb = show_toast.clone();
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
                        toast_cb.emit((format!("Download failed: {err}"), false));
                        return;
                    }
                }
            };

            let transfer_store = transfer_store.clone();
            let transfers_state = transfers_state.clone();
            let push_log = push_log.clone();
            let show_toast = toast_cb.clone();
            show_toast.emit(("Preparing download…".into(), true));
            wasm_bindgen_futures::spawn_local(async move {
                let result = transfer::finalize_and_download(&plan).await;
                {
                    let mut store = transfer_store.borrow_mut();
                    store.finish_finalize(transfer_id, &result);
                    transfers_state.set(store.snapshots());
                }
                match result {
                    Ok(()) => {
                        push_log.emit(format!("download ready: {}", plan.file_name));
                        show_toast.emit(("Download started".into(), true));
                    }
                    Err(err) => {
                        push_log.emit(format!("download failed: {err}"));
                        show_toast.emit((format!("Download failed: {err}"), false));
                    }
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
                toast_cb.emit((
                    "Choose a host path before starting a transfer".into(),
                    false,
                ));
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
                toast_cb.emit((
                    "Choose a host path before setting the default".into(),
                    false,
                ));
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

    let on_usb_stop = {
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
                    Ok(()) => {
                        push_log.emit("USB disabling request sent".into());
                        show_toast.emit(("USB disabling…".into(), true));
                    }
                    Err(e) => {
                        push_log.emit(format!("usb stop error: {e}"));
                        show_toast.emit((format!("USB stop failed: {e}"), false));
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
    let busy = *busy_state;
    let connected = *ws_connected;
    let current_section = *active_section;
    let select_section = |section: AppSection| {
        let active_section = active_section.clone();
        Callback::from(move |_| active_section.set(section))
    };
    let section_title = match current_section {
        AppSection::Overview => ("Overview", "Device health, identity, and USB bring-up"),
        AppSection::Scripts => ("Scripts", "Compose and run keyboard automation"),
        AppSection::Transfers => ("Transfers", "Browse the host and receive files securely"),
        AppSection::Diagnostics => ("Diagnostics", "Connection events and device activity"),
    };

    html! {
        <div class="app-shell">
            <ToastBar toast={(*toast).clone()} />
            <aside class="sidebar">
                <div class="brand">
                    <div class="brand-mark" aria-hidden="true">{"P"}</div>
                    <div>
                        <div class="brand-name">{"Pico Endpoint"}</div>
                        <div class="brand-subtitle">{"Device console"}</div>
                    </div>
                </div>
                <nav class="primary-nav" aria-label="Main navigation">
                    <button class={classes!("nav-item", (current_section == AppSection::Overview).then_some("active"))} onclick={select_section(AppSection::Overview)}>
                        <span class="nav-icon" aria-hidden="true">{"◫"}</span><span>{"Overview"}</span>
                    </button>
                    <button class={classes!("nav-item", (current_section == AppSection::Scripts).then_some("active"))} onclick={select_section(AppSection::Scripts)}>
                        <span class="nav-icon" aria-hidden="true">{"⌁"}</span><span>{"Scripts"}</span>
                    </button>
                    <button class={classes!("nav-item", (current_section == AppSection::Transfers).then_some("active"))} onclick={select_section(AppSection::Transfers)}>
                        <span class="nav-icon" aria-hidden="true">{"⇄"}</span><span>{"Transfers"}</span>
                    </button>
                    <button class={classes!("nav-item", (current_section == AppSection::Diagnostics).then_some("active"))} onclick={select_section(AppSection::Diagnostics)}>
                        <span class="nav-icon" aria-hidden="true">{"⌘"}</span><span>{"Diagnostics"}</span>
                    </button>
                </nav>
                <div class="sidebar-footer">
                    <span class={classes!("device-dot", connection_state.class())}></span>
                    <div><strong>{"RP2350 console"}</strong><span>{"Desktop interface"}</span></div>
                </div>
            </aside>

            <main class="workspace">
                <header class="topbar">
                    <div>
                        <h1>{section_title.0}</h1>
                        <p>{section_title.1}</p>
                    </div>
                    <div class="topbar-status">
                        <span class={classes!("connection-pill", connection_state.class())}>
                            <span class="status-dot"></span>{connection_state.label()}
                        </span>
                        <span class="topbar-metric"><span>{"USB"}</span><strong>{st.as_ref().map(|s| if s.usb_ready { "Ready" } else if s.usb_enabled { "Starting" } else { "Off" }).unwrap_or("Checking")}</strong></span>
                        <span class="topbar-metric"><span>{"Host"}</span><strong>{st.as_ref().map(|s| s.host_os.as_str()).unwrap_or("Unknown")}</strong></span>
                    </div>
                </header>

                <div class="content-area">
                    { match current_section {
                        AppSection::Overview => html! {
                            <div class="overview-grid">
                                <div class="overview-stack">
                                    <StatusCard status={st.clone()} busy={busy} connection={*connection_state} />
                                    <UsbCard
                                        selected_os={(*selected_os).clone()}
                                        on_select_os={{
                                            let selected_os = selected_os.clone();
                                            Callback::from(move |os: String| selected_os.set(os))
                                        }}
                                        usb_enabled={st.as_ref().map(|s| s.usb_enabled).unwrap_or(false)}
                                        connected={connected}
                                        busy={busy}
                                        on_start={on_usb_start.clone()}
                                        on_stop={on_usb_stop.clone()} />
                                </div>
                                <IdentityCard
                                    config={conf.clone()}
                                    usb_enabled={st.as_ref().map(|s| s.usb_enabled).unwrap_or(false)}
                                    connected={connected}
                                    busy={busy}
                                    on_save={on_save_identity.clone()} />
                            </div>
                        },
                        AppSection::Scripts => html! {
                            <ScriptingCard
                                dsl_text={(*dsl_text).clone()}
                                on_change={{
                                    let dsl_text = dsl_text.clone();
                                    Callback::from(move |s: String| dsl_text.set(s))
                                }}
                                on_select_layout={{
                                    let dsl_text = dsl_text.clone();
                                    Callback::from(move |layout: String| dsl_text.set(dsl::set_entry_layout(&dsl_text, &layout)))
                                }}
                                scripts={scr.clone()}
                                connected={connected}
                                busy={busy}
                                on_run={on_run_dsl.clone()} />
                        },
                        AppSection::Transfers => html! {
                            <div class="transfer-workspace">
                                <TransferStartCard
                                    path={(*transfer_path).clone()}
                                    connected={connected}
                                    busy={busy}
                                    on_change={{
                                        let transfer_path = transfer_path.clone();
                                        Callback::from(move |value: String| transfer_path.set(value))
                                    }}
                                    on_start={on_start_transfer.clone()}
                                    on_set_default={on_set_transfer_default.clone()} />
                                <FileBrowserCard
                                    view={(*filesystem_view).clone()}
                                    connected={connected}
                                    on_browse={on_browse.clone()}
                                    on_select={{
                                        let transfer_path = transfer_path.clone();
                                        Callback::from(move |path: String| transfer_path.set(path))
                                    }}
                                    on_transfer={queue_transfer_path.clone()} />
                                <DownloadManagerCard transfers={(*transfers).clone()} on_download={on_download_transfer.clone()} />
                            </div>
                        },
                        AppSection::Diagnostics => html! {
                            <LogCard
                                lines={(*log_lines).clone()}
                                connection={*connection_state}
                                on_clear={{
                                    let log_lines = log_lines.clone();
                                    Callback::from(move |_| log_lines.set(Vec::new()))
                                }} />
                        },
                    } }
                </div>
            </main>
        </div>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct StatusProps {
    pub status: Option<StatusState>,
    pub busy: bool,
    pub connection: ConnectionState,
}
#[function_component(StatusCard)]
fn status_card(props: &StatusProps) -> Html {
    let st = &props.status;
    html! {
        <section class="card status-card" aria-busy={props.busy.to_string()}>
            <div class="card-header">
                <div><span class="eyebrow">{"Live telemetry"}</span><h2>{"Device status"}</h2></div>
                if props.busy { <span class="spinner-label"><span class="spinner"></span>{"Updating"}</span> }
            </div>
            <div class="metric-grid">
                <div class="metric"><span>{"Connection"}</span><strong class={props.connection.class()}>{props.connection.label()}</strong></div>
                <div class="metric"><span>{"USB device"}</span><strong>{st.as_ref().map(|s| if s.usb_enabled { "Enabled" } else { "Off" }).unwrap_or("Checking…")}</strong></div>
                <div class="metric"><span>{"Host ready"}</span><strong>{st.as_ref().map(|s| if s.usb_ready { "Ready" } else { "Not ready" }).unwrap_or("Checking…")}</strong></div>
                <div class="metric"><span>{"Detected host"}</span><strong>{st.as_ref().map(|s| s.host_os.as_str()).unwrap_or("Unknown")}</strong></div>
            </div>
            <div class="card-footer-note"><span class="pulse-dot"></span>{"Status refreshes every five seconds while connected"}</div>
        </section>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct IdentityProps {
    pub config: Option<ConfigState>,
    pub usb_enabled: bool,
    pub connected: bool,
    pub busy: bool,
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
    let valid = man.is_ascii() && prod.is_ascii();
    let fields_disabled = props.usb_enabled || props.config.is_none() || !props.connected;
    let disabled = fields_disabled || props.busy || !dirty || !valid;
    let on_save = {
        let man = man.clone();
        let prod = prod.clone();
        let cb = props.on_save.clone();
        Callback::from(move |_| cb.emit(((*man).clone(), (*prod).clone())))
    };
    html! {
        <section class="card identity-card" id="identityCard">
          <div class="card-header">
            <div><span class="eyebrow">{"USB descriptor"}</span><h2>{"Device identity"}</h2></div>
            <span class={classes!("lock-state", props.usb_enabled.then_some("locked"))}>{if props.usb_enabled { "Locked while USB is active" } else if !props.connected { "Unavailable" } else if props.config.is_none() { "Loading" } else { "Editable" }}</span>
          </div>
          if props.config.is_none() {
            <div class="inline-notice">{if props.connected { "Loading device configuration…" } else { "Connect to the device to load its identity." }}</div>
          }
          <div class="identity-fields">
            <label class="field flex-1">
              <div>
                <span>{"USB manufacturer"}</span>
                <input id="usbManufacturer" type="text" placeholder={if props.config.is_some() { "Pico 2W" } else { "Waiting for device…" }} maxlength="32"
                  value={(*man).clone()}
                  oninput={{ let man=man.clone(); Callback::from(move |e: InputEvent| man.set(e.target_unchecked_into::<web_sys::HtmlInputElement>().value())) }}
                  disabled={fields_disabled} />
                <div class="row between">
                  <span class="hint">{"ASCII only, max 32 bytes."}</span>
                  <span class="hint">{ if props.config.is_some() { format!("{}/32", (*man).len()) } else { "—".into() } }</span>
                </div>
              </div>
            </label>
            <label class="field flex-2">
              <div>
                <span>{"USB product string"}</span>
                <input id="usbProduct" type="text" placeholder={if props.config.is_some() { "Logger + Keyboard" } else { "Waiting for device…" }} maxlength="48"
                  value={(*prod).clone()}
                  oninput={{ let prod=prod.clone(); Callback::from(move |e: InputEvent| prod.set(e.target_unchecked_into::<web_sys::HtmlInputElement>().value())) }}
                  disabled={fields_disabled} />
                <div class="row between">
                  <span class="hint">{"ASCII only, max 48 bytes."}</span>
                  <span class="hint">{ if props.config.is_some() { format!("{}/48", (*prod).len()) } else { "—".into() } }</span>
                </div>
              </div>
            </label>
          </div>
          <div class="card-actions">
            <span class={classes!("form-note", (!valid).then_some("error"))}>{if valid { "Changes apply the next time USB starts." } else { "Only ASCII characters are supported." }}</span>
            <button id="btnSaveIdentity" class="btn-primary" {disabled} onclick={on_save}>{if props.busy { "Saving…" } else { "Save identity" }}</button>
          </div>
        </section>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct UsbProps {
    pub selected_os: String,
    pub on_select_os: Callback<String>,
    pub on_start: Callback<bool>,
    pub on_stop: Callback<()>,
    pub usb_enabled: bool,
    pub connected: bool,
    pub busy: bool,
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
        <section class="card usb-card" id="usbCard">
          <div class="card-header"><div><span class="eyebrow">{"Composite device"}</span><h2>{"USB bring-up"}</h2></div></div>
          if !props.usb_enabled {
            <>
              <p class="card-copy">{"Choose the target host before registering the keyboard and storage interfaces."}</p>
              <div class="row radio-bar mb-1">
                <label class="radio"><input type="radio" name="os" value="mac" checked={props.selected_os=="mac"} onclick={set_mac}/>{" macOS"}</label>
                <label class="radio"><input type="radio" name="os" value="windows" checked={props.selected_os=="windows"} onclick={set_win}/>{" Windows"}</label>
              </div>
              <div class="card-actions left">
                <button id="btnUsbAssistant" class="btn-primary" onclick={start_assist} disabled={!props.connected || props.busy || props.selected_os=="windows"}>{"Start with Assistant"}</button>
                <button id="btnUsbNoAssistant" class="btn-secondary" onclick={start_noassist} disabled={!props.connected || props.busy}>{"Start USB"}</button>
              </div>
              <div class="card-footer-note">{"Assistant runs the macOS keyboard identification sequence after registration."}</div>
            </>
          } else {
            <div class="active-state"><span class="active-state-icon">{"✓"}</span><div><strong>{"USB is active"}</strong><span>{"The device is registered with the host."}</span></div></div>
            <div class="card-actions left">
              <button id="btnUsbStop" class="btn-danger" disabled={!props.connected || props.busy} onclick={{ let cb = props.on_stop.clone(); Callback::from(move |_| cb.emit(())) }}>{"Stop USB"}</button>
            </div>
          }
        </section>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct ScriptProps {
    pub dsl_text: String,
    pub on_change: Callback<String>,
    pub on_select_layout: Callback<String>,
    pub scripts: Option<Vec<api::ScriptMeta>>,
    pub connected: bool,
    pub busy: bool,
    pub on_run: Callback<()>,
}
#[function_component(ScriptingCard)]
fn scripting_card(props: &ScriptProps) -> Html {
    let search = use_state(String::new);
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

    let layouts = dsl_core::available_layouts();
    let selected_layout = dsl::entry_layout(&props.dsl_text).unwrap_or("");
    let line_count = props.dsl_text.lines().count();
    let can_run = props.connected
        && !props.busy
        && !selected_layout.is_empty()
        && line_count <= MAX_DSL_LINES;

    let on_keydown = {
        let cb = props.on_run.clone();
        Callback::from(move |e: KeyboardEvent| {
            if can_run && (e.ctrl_key() || e.meta_key()) && e.key() == "Enter" {
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

    let query = search.trim().to_ascii_lowercase();

    html! {
      <div class="script-workspace">
        <section class="card editor-card">
          <div class="card-header editor-header">
            <div><span class="eyebrow">{"DSL editor"}</span><h2>{"Automation script"}</h2></div>
            <div class="editor-meta">
              <label for="scriptLayout">{"Keyboard layout"}</label>
              <select id="scriptLayout" required=true value={selected_layout.to_string()} onchange={on_layout}>
                <option value="" disabled=true selected={selected_layout.is_empty()}>{"Select layout…"}</option>
                { for layouts.iter().map(|id| html!{ <option value={id.to_string()}>{ *id }</option> }) }
              </select>
              <span class={classes!("line-count", (line_count > MAX_DSL_LINES).then_some("error"))}>{format!("{line_count} / {MAX_DSL_LINES} lines")}</span>
            </div>
          </div>
          <textarea id="scriptDsl" spellcheck="false" aria-label="Automation script" value={props.dsl_text.clone()} oninput={on_text} onkeydown={on_keydown} />
          <div class="command-reference">
            <span>{"Commands"}</span>
            <code>{"tap(\"KEY\")"}</code><code>{"modtap(\"MOD+KEY\")"}</code><code>{"delay(MS)"}</code><code>{"text(\"STRING\", DELAY)"}</code>
          </div>
          <div class="card-actions editor-actions">
            <span class="form-note">{if props.connected { "Press Ctrl/⌘ + Enter to run" } else { "Connect to the device before running scripts" }}</span>
            <button id="btnRunDsl" class="btn-primary run-button" disabled={!can_run} onclick={{ let cb=props.on_run.clone(); Callback::from(move |_| cb.emit(())) }}>
              <span aria-hidden="true">{"▶"}</span>{if props.busy { "Running…" } else { "Run script" }}
            </button>
          </div>
        </section>

        <aside class="card library-card">
          <div class="card-header"><div><span class="eyebrow">{"On-device library"}</span><h2>{"Examples"}</h2></div></div>
          <input class="library-search" type="search" placeholder="Search scripts…" value={(*search).clone()}
            oninput={{ let search=search.clone(); Callback::from(move |e: InputEvent| search.set(e.target_unchecked_into::<web_sys::HtmlInputElement>().value())) }} />
          <div class="library-list">
            if let Some(list) = &props.scripts {
              { for list.iter().filter(|script| query.is_empty() || script.name.to_ascii_lowercase().contains(&query) || script.description.to_ascii_lowercase().contains(&query)).map(|s| html!{
                <article class="script-item">
                  <div><strong>{&s.name}</strong><p>{&s.description}</p></div>
                  <div>
                    if let Some(pre) = &s.dsl {
                      <button class="btn-quiet" onclick={insert_script(pre.clone())}>{"Load"}</button>
                    }
                  </div>
                </article>
              }) }
            } else {
              <div class="inline-notice">{"Loading script library…"}</div>
            }
          </div>
        </aside>
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
        <section class="card file-browser">
          <div class="card-header">
            <div><span class="eyebrow">{"Host filesystem"}</span><h2>{"Source files"}</h2></div>
            <span class={classes!("connection-caption", props.connected.then_some("connected"))}>
              { if props.connected { "Host filesystem via USB agent" } else { "WebSocket disconnected" } }
            </span>
          </div>

          <div class="file-toolbar">
            <button class="btn-quiet" onclick={browse_home} disabled={!props.connected || props.view.loading}>{"Home"}</button>
            <button class="btn-quiet" onclick={browse_root} disabled={!props.connected || props.view.loading}>{"Root"}</button>
            <button
                class="btn-quiet"
                onclick={browse_parent}
                disabled={!props.connected || props.view.loading || props.view.directory.is_empty() || props.view.directory == "/"}
            >
              {"Up"}
            </button>
            <button class="btn-quiet" onclick={refresh} disabled={!props.connected || props.view.loading}>{"Refresh"}</button>
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
            if !props.connected {
              <div class="empty-state"><strong>{"Device connection required"}</strong><span>{"Connect the Pico and host-agent to browse files."}</span></div>
            } else if props.view.entries.is_empty() && !props.view.loading && props.view.error.is_none() {
              <div class="empty-state"><strong>{"This directory is empty"}</strong><span>{"Choose another folder or enable hidden files."}</span></div>
            }
            { for props.view.entries.iter().map(|entry| {
                let full_path = filesystem::join_path(&props.view.directory, &entry.name);
                let icon = match entry.kind {
                    filesystem::EntryKind::Directory => "DIR",
                    filesystem::EntryKind::SymlinkDirectory => "LNK",
                    filesystem::EntryKind::File => "FILE",
                    filesystem::EntryKind::SymlinkFile => "LNK",
                    filesystem::EntryKind::Other => "—",
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
                      <span class="file-kind">{icon}</span>
                      <span>{&entry.name}</span>
                    </button>
                    <span class="fs-meta">
                      { if entry.kind.is_file() { format_bytes(entry.size) } else { "—".to_string() } }
                    </span>
                    <span class="fs-meta">{format_modified(entry.modified_secs)}</span>
                    <span class="row gap-1">
                      if entry.kind.is_file() {
                        <button class="btn-secondary" onclick={select} disabled={!entry.readable}>{"Select"}</button>
                        <button class="btn-primary btn-small" onclick={transfer} disabled={!entry.readable}>{"Transfer"}</button>
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
        </section>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct TransferStartProps {
    pub path: String,
    pub connected: bool,
    pub busy: bool,
    pub on_change: Callback<String>,
    pub on_start: Callback<()>,
    pub on_set_default: Callback<()>,
}

#[function_component(TransferStartCard)]
fn transfer_start_card(props: &TransferStartProps) -> Html {
    let disabled = !props.connected || props.busy || props.path.trim().is_empty();
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
        <section class="card transfer-start-card">
          <div class="card-header"><div><span class="eyebrow">{"Quick transfer"}</span><h2>{"Queue a host path"}</h2></div></div>
          <div class="transfer-path-row">
            <input
                id="transferPath"
                type="text"
                placeholder="/Users/alice/Downloads/archive.zip"
                value={props.path.clone()}
                oninput={on_input}
            />
            <button class="btn-primary" disabled={disabled} onclick={on_start}>{if props.busy { "Queueing…" } else { "Queue transfer" }}</button>
            <button class="btn-secondary" disabled={disabled} onclick={on_set_default}>{"Set as default"}</button>
          </div>
          <div class="card-footer-note">{if props.connected { "Paths are resolved on the host-agent machine." } else { "Connect to the device before queueing a transfer." }}</div>
        </section>
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
        <section class="card download-card">
          <div class="card-header">
            <div><span class="eyebrow">{"Transfer activity"}</span><h2>{"Downloads"}</h2></div>
            <span class="count-pill">{ format!("{} transfer(s)", props.transfers.len()) }</span>
          </div>
          if props.transfers.is_empty() {
            <div class="empty-state compact"><strong>{"No active transfers"}</strong><span>{"Files queued from the browser will appear here."}</span></div>
          } else {
            <div class="transfer-list">
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
                    <div class="transfer-row">
                      <div class="transfer-row-main">
                        <div>
                          <strong>{ &transfer.file_name }</strong>
                          <div class="transfer-subtitle">{ format!("Transfer {} · {}", transfer.transfer_id, transfer_status_label(&transfer.status)) }</div>
                        </div>
                        <div class="row gap-2 items-center">
                          <span>{ format!("{} / {}", format_bytes(transfer.received_size), format_bytes(transfer.total_size)) }</span>
                          <span>{ format_rate(transfer.smoothed_rate_bps) }</span>
                          <span>{ format_eta(transfer.eta_total_secs) }</span>
                          <button
                            class="btn-secondary"
                            disabled={transfer.status != transfer::TransferState::Finished}
                            onclick={on_download}
                          >
                            {"Download"}
                          </button>
                        </div>
                      </div>
                      <div class="progress-track"><span style={format!("width: {progress_pct:.1}%")}></span></div>
                      <div class="transfer-details">{ format!(
                        "{:.1}% · chunks {} · finished {} · failed {} · retrying {}",
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
        </section>
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
    pub connection: ConnectionState,
    pub on_clear: Callback<()>,
}
#[function_component(LogCard)]
fn log_card(props: &LogProps) -> Html {
    let filter = use_state(String::new);
    let errors_only = use_state(|| false);
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
    let query = filter.trim().to_ascii_lowercase();
    let visible_lines = props.lines.iter().filter(|line| {
        let normalized = line.to_ascii_lowercase();
        (query.is_empty() || normalized.contains(&query))
            && (!*errors_only
                || normalized.contains("error")
                || normalized.contains("failed")
                || normalized.contains("lost"))
    });
    html! {
        <section class="card diagnostics-card">
          <div class="card-header">
            <div><span class="eyebrow">{"Runtime events"}</span><h2>{"Activity log"}</h2></div>
            <span class={classes!("connection-pill", props.connection.class())}><span class="status-dot"></span>{props.connection.label()}</span>
          </div>
          <div class="log-toolbar">
            <input type="search" placeholder="Filter log entries…" value={(*filter).clone()}
              oninput={{ let filter=filter.clone(); Callback::from(move |e: InputEvent| filter.set(e.target_unchecked_into::<web_sys::HtmlInputElement>().value())) }} />
            <label class="inline toggle-control"><input type="checkbox" checked={*errors_only} onchange={{ let errors_only=errors_only.clone(); Callback::from(move |e: Event| errors_only.set(e.target_unchecked_into::<web_sys::HtmlInputElement>().checked())) }} />{"Errors only"}</label>
            <button id="btnClearLog" class="btn-secondary" onclick={{ let cb=props.on_clear.clone(); Callback::from(move |_| cb.emit(())) }}>{"Clear log"}</button>
          </div>
          <div id="log" ref={node_ref}>
            if props.lines.is_empty() {
              <div class="log-empty">{"No events recorded in this session."}</div>
            } else {
              { for visible_lines.enumerate().map(|(index, line)| html!{ <div class="log-entry"><span>{format!("{:03}", index + 1)}</span><code>{line}</code></div> }) }
            }
          </div>
        </section>
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
