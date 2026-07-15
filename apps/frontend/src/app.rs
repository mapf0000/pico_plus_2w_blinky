use super::*;

#[derive(Clone, PartialEq)]
pub(crate) struct StatusState {
    pub(crate) usb_enabled: bool,
    pub(crate) usb_ready: bool,
    pub(crate) host_os: String,
}

#[derive(Clone, PartialEq)]
pub(crate) struct ConfigState {
    pub(crate) usb_manufacturer: String,
    pub(crate) usb_product: String,
}

#[derive(Clone)]
pub(crate) struct BrowseRequest {
    pub(crate) path: String,
    pub(crate) cursor: u32,
    pub(crate) append: bool,
    pub(crate) show_hidden: bool,
}

const STARTER_SCRIPT: &str = "layout(\"mac_de-DE\")\nmodtap(\"LGUI+SPACE\")\ndelay(400)\ntext(\"Terminal\", 10)\ntap(\"ENTER\")";

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum AppSection {
    Overview,
    Scripts,
    Transfers,
    Diagnostics,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum ConnectionState {
    Connecting,
    Connected,
    Reconnecting,
    Unavailable,
}

#[derive(Clone, Copy)]
enum PendingAction {
    Refresh,
    SaveIdentity,
    UsbStart,
    UsbStop,
    RunScript,
    TransferStart,
    TransferDefault,
}

#[derive(Clone, Default, PartialEq)]
struct PendingActions {
    refresh: bool,
    save_identity: bool,
    usb_start: bool,
    usb_stop: bool,
    run_script: bool,
    transfer_start: bool,
    transfer_default: bool,
}

impl PendingActions {
    fn set(&mut self, action: PendingAction, pending: bool) {
        match action {
            PendingAction::Refresh => self.refresh = pending,
            PendingAction::SaveIdentity => self.save_identity = pending,
            PendingAction::UsbStart => self.usb_start = pending,
            PendingAction::UsbStop => self.usb_stop = pending,
            PendingAction::RunScript => self.run_script = pending,
            PendingAction::TransferStart => self.transfer_start = pending,
            PendingAction::TransferDefault => self.transfer_default = pending,
        }
    }
}

impl ConnectionState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Reconnecting => "Reconnecting",
            Self::Unavailable => "Unavailable",
        }
    }

    pub(crate) fn class(self) -> &'static str {
        match self {
            Self::Connected => "is-good",
            Self::Connecting | Self::Reconnecting => "is-warn",
            Self::Unavailable => "is-bad",
        }
    }
}

#[function_component(App)]
pub(crate) fn app() -> Html {
    let status: UseStateHandle<Option<StatusState>> = use_state(|| None);
    let config: UseStateHandle<Option<ConfigState>> = use_state(|| None);
    let scripts: UseStateHandle<Option<Vec<api::ScriptMeta>>> = use_state(|| None);
    let transfer_store = use_mut_ref(transfer::TransferStore::new);
    let transfers = use_state(Vec::<transfer::TransferView>::new);
    let transfer_path = use_state(String::new);
    let secure_session = use_mut_ref(transfer::SecureSession::new);
    let secure_session_view = use_state(|| transfer::SecureSessionView::Idle);
    let filesystem_store = use_mut_ref(filesystem::BrowserStore::new);
    let filesystem_view = use_state(filesystem::BrowserView::default);
    let dsl_text = use_state(|| STARTER_SCRIPT.to_string());
    let selected_os = use_state(|| String::from("mac"));
    let active_section = use_state(|| AppSection::Overview);
    let pending_actions = use_state(PendingActions::default);
    let log_lines = use_state(Vec::<String>::new);
    let ws_connected = use_state(|| false);
    let hello = use_state(|| None::<api::Hello>);
    let handshake_error = use_state(|| None::<String>);
    let connection_state = use_state(|| ConnectionState::Connecting);
    let was_connected = use_mut_ref(|| false);
    let toast = use_state(|| None::<(String, bool)>); // (message, ok?)
    // All WebSocket API calls are handled in api.rs via a single connection

    let set_pending = {
        let pending_actions = pending_actions.clone();
        Callback::from(move |(action, pending): (PendingAction, bool)| {
            let mut next = (*pending_actions).clone();
            next.set(action, pending);
            pending_actions.set(next);
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
                    store.fail_request(request_id, error.to_string());
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
        let hello = hello.clone();
        let handshake_error = handshake_error.clone();
        let set_pending = set_pending.clone();
        let push_log = push_log.clone();
        use_effect_with(*ws_connected, move |connected| {
            if *connected {
                let status = status.clone();
                let config = config.clone();
                let hello = hello.clone();
                let handshake_error = handshake_error.clone();
                let set_pending = set_pending.clone();
                let push_log = push_log.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    set_pending.emit((PendingAction::Refresh, true));
                    match api::get_hello().await {
                        Ok(snapshot) => {
                            handshake_error.set(snapshot.compatibility_error());
                            hello.set(Some(snapshot));
                        }
                        Err(error) => {
                            let message = format!(
                                "Capability handshake failed ({}): {error}",
                                error.category()
                            );
                            handshake_error.set(Some(message.clone()));
                            push_log.emit(message);
                        }
                    }
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
                    set_pending.emit((PendingAction::Refresh, false));
                });
            } else {
                hello.set(None);
                handshake_error.set(None);
            }
            || ()
        });
    }

    // Poll status only while connected; reconnecting does its own refresh.
    {
        let status = status.clone();
        let hello = hello.clone();
        let handshake_error = handshake_error.clone();
        let push_log = push_log.clone();
        use_effect_with(*ws_connected, move |connected| {
            let handle = connected.then(|| {
                gloo_timers::callback::Interval::new(5000, move || {
                    let status = status.clone();
                    let hello = hello.clone();
                    let handshake_error = handshake_error.clone();
                    let push_log = push_log.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        match api::get_hello().await {
                            Ok(snapshot) => {
                                handshake_error.set(snapshot.compatibility_error());
                                hello.set(Some(snapshot));
                            }
                            Err(error) => push_log.emit(format!(
                                "HELLO refresh error ({}): {error}",
                                error.category()
                            )),
                        }
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
        let hello_state = hello.clone();
        let handshake_error = handshake_error.clone();
        let pending_actions = pending_actions.clone();
        let connection_state = connection_state.clone();
        let was_connected = was_connected.clone();
        let transfer_store = transfer_store.clone();
        let transfers_state = transfers.clone();
        let secure_session = secure_session.clone();
        let secure_session_view = secure_session_view.clone();
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
                    let hello_state = hello_state.clone();
                    let handshake_error = handshake_error.clone();
                    let pending_actions = pending_actions.clone();
                    let secure_session = secure_session.clone();
                    let secure_session_view = secure_session_view.clone();
                    move |connected| {
                        ws_connected.set(connected);
                        if connected {
                            *was_connected.borrow_mut() = true;
                            connection_state.set(ConnectionState::Connected);
                        } else if *was_connected.borrow() {
                            secure_session.borrow_mut().reset();
                            secure_session_view.set(transfer::SecureSessionView::Idle);
                            hello_state.set(None);
                            handshake_error.set(None);
                            pending_actions.set(PendingActions::default());
                            connection_state.set(ConnectionState::Reconnecting);
                        } else {
                            secure_session.borrow_mut().reset();
                            secure_session_view.set(transfer::SecureSessionView::Idle);
                            hello_state.set(None);
                            pending_actions.set(PendingActions::default());
                            connection_state.set(ConnectionState::Unavailable);
                        }
                    }
                },
                {
                    let hello_state = hello_state.clone();
                    let handshake_error = handshake_error.clone();
                    let push_log = push_log.clone();
                    move |snapshot: api::Hello| {
                        let compatibility_error = snapshot.compatibility_error();
                        push_log.emit(format!(
                            "HELLO firmware={} build={} ws={} transfer={} filesystem={} agent={}",
                            snapshot.firmware.version,
                            snapshot.firmware.build,
                            snapshot.protocols.websocket,
                            snapshot.protocols.transfer,
                            snapshot.protocols.filesystem,
                            snapshot.host_agent.version.as_deref().unwrap_or(
                                if snapshot.host_agent.present {
                                    "legacy"
                                } else {
                                    "absent"
                                }
                            )
                        ));
                        handshake_error.set(compatibility_error);
                        hello_state.set(Some(snapshot));
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
                    let secure_session = secure_session.clone();
                    let secure_session_view = secure_session_view.clone();
                    move |kind, binary| {
                        let result = (|| -> Result<(), String> {
                            match kind {
                                transfer_protocol::WS_BINARY_KIND_SESSION => {
                                    let outbound =
                                        secure_session.borrow_mut().handle_session(&binary)?;
                                    let view = secure_session.borrow().view();
                                    secure_session_view.set(view);
                                    if view.established() {
                                        push_log.emit(
                                            "encrypted file-transfer session established".into(),
                                        );
                                    }
                                    if let Some(outbound) = outbound {
                                        api::send_secure_transfer(&outbound)
                                            .map_err(|error| error.to_string())?;
                                    }
                                }
                                transfer_protocol::WS_BINARY_KIND_SECURE_OPEN => {
                                    let open = secure_session.borrow_mut().decrypt_open(&binary)?;
                                    let now_ms = monotonic_now_ms();
                                    let mut store = transfer_store.borrow_mut();
                                    store.apply_secure_open(open, now_ms)?;
                                    transfers_state.set(store.snapshots());
                                }
                                transfer_protocol::WS_BINARY_KIND_SECURE_CHUNK => {
                                    let (transfer_id, chunk_index, plaintext) =
                                        secure_session.borrow().decrypt_chunk(&binary)?;
                                    let now_ms = monotonic_now_ms();
                                    let mut store = transfer_store.borrow_mut();
                                    store.apply_secure_chunk(
                                        transfer_id,
                                        chunk_index,
                                        plaintext.as_slice(),
                                        now_ms,
                                    )?;
                                }
                                transfer_protocol::WS_BINARY_KIND_SECURE_CHUNK_BATCH => {
                                    let batch =
                                        transfer_protocol::decode_secure_chunk_batch(&binary)
                                            .map_err(|_| {
                                                "invalid encrypted chunk batch".to_string()
                                            })?;
                                    let now_ms = monotonic_now_ms();
                                    let session = secure_session.borrow();
                                    let mut store = transfer_store.borrow_mut();
                                    for chunk in batch.chunks() {
                                        let (transfer_id, chunk_index, plaintext) =
                                            session.decrypt_chunk(chunk)?;
                                        store.apply_secure_chunk(
                                            transfer_id,
                                            chunk_index,
                                            plaintext.as_slice(),
                                            now_ms,
                                        )?;
                                    }
                                }
                                transfer_protocol::WS_BINARY_KIND_SECURE_CLOSE => {
                                    let close =
                                        secure_session.borrow_mut().decrypt_close(&binary)?;
                                    let transfer_id = close.transfer_id;
                                    let now_ms = monotonic_now_ms();
                                    let digest = {
                                        let mut store = transfer_store.borrow_mut();
                                        let digest = store.apply_secure_close(close, now_ms)?;
                                        transfers_state.set(store.snapshots());
                                        digest
                                    };
                                    let receipt = secure_session
                                        .borrow_mut()
                                        .seal_receipt(transfer_id, &digest)?;
                                    api::send_secure_transfer(&receipt)
                                        .map_err(|error| error.to_string())?;
                                    push_log.emit(format!(
                                        "encrypted transfer {transfer_id} authenticated end to end"
                                    ));
                                }
                                _ => return Err(format!("unknown encrypted transfer kind {kind}")),
                            }
                            Ok(())
                        })();
                        if let Err(error) = result {
                            if let Some(transfer_id) = transfer::secure_transfer_id(kind, &binary) {
                                secure_session.borrow_mut().discard_file(transfer_id);
                                let mut store = transfer_store.borrow_mut();
                                store.fail_secure_transfer(transfer_id, monotonic_now_ms());
                                transfers_state.set(store.snapshots());
                            }
                            secure_session_view.set(secure_session.borrow().view());
                            push_log.emit(format!("encrypted transfer error: {error}"));
                        }
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
        let set_pending = set_pending.clone();
        let push_log = push_log.clone();
        let toast_cb = show_toast.clone();
        Callback::from(move |(man, prod): (String, String)| {
            let config = config.clone();
            let set_pending = set_pending.clone();
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
                set_pending.emit((PendingAction::SaveIdentity, true));
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
                set_pending.emit((PendingAction::SaveIdentity, false));
            });
        })
    };

    let on_usb_start =
        {
            let selected_os = selected_os.clone();
            let set_pending = set_pending.clone();
            let push_log = push_log.clone();
            let toast_cb = show_toast.clone();
            Callback::from(move |assistant: bool| {
                let selected_os = (*selected_os).clone();
                let set_pending = set_pending.clone();
                let push_log = push_log.clone();
                let show_toast = toast_cb.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    set_pending.emit((PendingAction::UsbStart, true));
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
                    set_pending.emit((PendingAction::UsbStart, false));
                });
            })
        };

    let on_run_dsl = {
        let dsl_text = dsl_text.clone();
        let set_pending = set_pending.clone();
        let push_log = push_log.clone();
        let toast_cb = show_toast.clone();
        Callback::from(move |_| {
            let txt = (*dsl_text).clone();
            if txt.trim().is_empty() {
                push_log.emit("empty script".to_string());
                toast_cb.emit(("Add commands before running the script".into(), false));
                return;
            }
            let set_pending = set_pending.clone();
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
                set_pending.emit((PendingAction::RunScript, true));
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
                set_pending.emit((PendingAction::RunScript, false));
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
        let secure_session = secure_session.clone();
        let set_pending = set_pending.clone();
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
            set_pending.emit((PendingAction::TransferStart, true));
            let result = secure_session
                .borrow_mut()
                .seal_start(&path)
                .map_err(api::ApiError::Protocol)
                .and_then(|payload| api::send_secure_transfer(&payload));
            match result {
                Ok(()) => {
                    push_log.emit("encrypted transfer request queued".into());
                    toast_cb.emit(("Transfer queued securely".into(), true));
                }
                Err(err) => {
                    push_log.emit(format!("transfer start error: {err}"));
                    toast_cb.emit((format!("Transfer start failed: {err}"), false));
                }
            }
            set_pending.emit((PendingAction::TransferStart, false));
        })
    };

    let on_start_transfer = {
        let transfer_path = transfer_path.clone();
        let queue_transfer_path = queue_transfer_path.clone();
        Callback::from(move |_| queue_transfer_path.emit((*transfer_path).clone()))
    };

    let on_set_transfer_default = {
        let secure_session = secure_session.clone();
        let transfer_path = transfer_path.clone();
        let set_pending = set_pending.clone();
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
            set_pending.emit((PendingAction::TransferDefault, true));
            let result = secure_session
                .borrow_mut()
                .seal_default_path(&path)
                .map_err(api::ApiError::Protocol)
                .and_then(|payload| api::send_secure_transfer(&payload));
            match result {
                Ok(()) => {
                    push_log.emit("encrypted default transfer path updated".into());
                    toast_cb.emit(("Transfer default updated securely".into(), true));
                }
                Err(err) => {
                    push_log.emit(format!("transfer default error: {err}"));
                    toast_cb.emit((format!("Set default failed: {err}"), false));
                }
            }
            set_pending.emit((PendingAction::TransferDefault, false));
        })
    };

    let on_start_default_transfer = {
        let secure_session = secure_session.clone();
        let set_pending = set_pending.clone();
        let push_log = push_log.clone();
        let toast_cb = show_toast.clone();
        Callback::from(move |_| {
            set_pending.emit((PendingAction::TransferStart, true));
            let result = secure_session
                .borrow_mut()
                .seal_start_default()
                .map_err(api::ApiError::Protocol)
                .and_then(|payload| api::send_secure_transfer(&payload));
            match result {
                Ok(()) => {
                    push_log.emit("encrypted default transfer request queued".into());
                    toast_cb.emit(("Default transfer queued securely".into(), true));
                }
                Err(err) => {
                    push_log.emit(format!("default transfer start error: {err}"));
                    toast_cb.emit((format!("Default transfer failed: {err}"), false));
                }
            }
            set_pending.emit((PendingAction::TransferStart, false));
        })
    };

    let on_request_session = {
        let secure_session = secure_session.clone();
        let secure_session_view = secure_session_view.clone();
        let push_log = push_log.clone();
        let toast_cb = show_toast.clone();
        Callback::from(move |_| {
            let result = secure_session
                .borrow_mut()
                .request_session()
                .map_err(api::ApiError::Protocol)
                .and_then(|payload| api::send_secure_transfer(&payload));
            secure_session_view.set(secure_session.borrow().view());
            match result {
                Ok(()) => push_log.emit("started unattended host negotiation".into()),
                Err(error) => toast_cb.emit((format!("Host connection failed: {error}"), false)),
            }
        })
    };

    let on_usb_stop = {
        let status = status.clone();
        let set_pending = set_pending.clone();
        let push_log = push_log.clone();
        let toast_cb = show_toast.clone();
        Callback::from(move |_| {
            let status = status.clone();
            let set_pending = set_pending.clone();
            let push_log = push_log.clone();
            let show_toast = toast_cb.clone();
            wasm_bindgen_futures::spawn_local(async move {
                set_pending.emit((PendingAction::UsbStop, true));
                match api::usb_unregister().await {
                    Ok(()) => {
                        let next_status = (*status).clone().map(|mut state| {
                            state.usb_enabled = false;
                            state.usb_ready = false;
                            state
                        });
                        status.set(next_status);
                        push_log.emit("USB disabled".into());
                        show_toast.emit(("USB disabled".into(), true));
                    }
                    Err(e) => {
                        push_log.emit(format!("usb stop error: {e}"));
                        show_toast.emit((format!("USB stop failed: {e}"), false));
                    }
                }
                set_pending.emit((PendingAction::UsbStop, false));
            });
        })
    };

    // UI pieces
    let st = (*status).clone();
    let conf = (*config).clone();
    let scr = (*scripts).clone();
    let pending = (*pending_actions).clone();
    let capabilities = (*hello).clone();
    let connected = *ws_connected;
    let websocket_compatible = capabilities
        .as_ref()
        .is_some_and(|snapshot| snapshot.compatibility_error().is_none());
    let device_ready = connected && websocket_compatible;
    let usb_control_ready = device_ready
        && capabilities
            .as_ref()
            .is_some_and(|snapshot| snapshot.supports_feature("usb_control"));
    let script_ready = device_ready
        && capabilities
            .as_ref()
            .is_some_and(|snapshot| snapshot.supports_keyboard_feature("script_bytecode"));
    let transfer_capable = device_ready
        && capabilities.as_ref().is_some_and(|snapshot| {
            snapshot.transfer_compatible()
                && snapshot.host_agent.present
                && snapshot.host_agent.version.is_some()
        });
    let transfer_ready = transfer_capable && secure_session_view.established();
    {
        let on_request_session = on_request_session.clone();
        use_effect_with(
            (transfer_capable, *secure_session_view),
            move |(capable, view)| {
                if *capable && *view == transfer::SecureSessionView::Idle {
                    on_request_session.emit(());
                }
                let retry = (*capable
                    && matches!(
                        *view,
                        transfer::SecureSessionView::Negotiating
                            | transfer::SecureSessionView::Handshaking
                    ))
                .then(|| {
                    gloo_timers::callback::Timeout::new(10_000, move || {
                        on_request_session.emit(());
                    })
                });
                move || drop(retry)
            },
        );
    }
    let filesystem_ready = device_ready
        && capabilities.as_ref().is_some_and(|snapshot| {
            snapshot.filesystem_compatible()
                && snapshot.host_agent.present
                && snapshot.host_agent.version.is_some()
        });
    let supported_layouts: Vec<String> = capabilities
        .as_ref()
        .map(|snapshot| {
            dsl_core::available_layouts()
                .iter()
                .copied()
                .filter(|layout| snapshot.supports_layout(layout))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let version_warning = capabilities.as_ref().and_then(|snapshot| {
        snapshot.compatibility_error().or_else(|| {
            (snapshot.protocols.transfer != api::TRANSFER_PROTOCOL_VERSION
                || snapshot.protocols.filesystem != api::FILESYSTEM_PROTOCOL_VERSION)
                .then(|| {
                    format!(
                        "firmware protocols are transfer={} filesystem={}; this UI expects transfer={} filesystem={}",
                        snapshot.protocols.transfer,
                        snapshot.protocols.filesystem,
                        api::TRANSFER_PROTOCOL_VERSION,
                        api::FILESYSTEM_PROTOCOL_VERSION
                    )
                })
        })
    });
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
                    <div>
                        <strong>{capabilities.as_ref().map(|snapshot| format!("Firmware {}", snapshot.firmware.version)).unwrap_or_else(|| "RP2350 console".into())}</strong>
                        <span>{capabilities.as_ref().map(|snapshot| snapshot.firmware.build.clone()).unwrap_or_else(|| "Desktop interface".into())}</span>
                    </div>
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
                    if connected && capabilities.is_none() && handshake_error.is_none() {
                        <div class="compatibility-banner is-loading"><strong>{"Negotiating capabilities"}</strong><span>{"Waiting for the firmware HELLO response…"}</span></div>
                    } else if let Some(message) = version_warning.as_ref().or(handshake_error.as_ref()) {
                        <div class="compatibility-banner is-error"><strong>{"Firmware update required"}</strong><span>{message}</span></div>
                    } else if current_section == AppSection::Transfers && capabilities.as_ref().is_some_and(|snapshot| !snapshot.host_agent.present) {
                        <div class="compatibility-banner is-warning"><strong>{"Host agent unavailable"}</strong><span>{"Start or update the USB host-agent to browse and transfer files."}</span></div>
                    } else if current_section == AppSection::Transfers && capabilities.as_ref().is_some_and(|snapshot| snapshot.host_agent.present && snapshot.host_agent.version.is_none()) {
                        <div class="compatibility-banner is-warning"><strong>{"Host agent update required"}</strong><span>{"The connected agent does not report a compatible version."}</span></div>
                    }
                    { match current_section {
                        AppSection::Overview => html! {
                            <div class="overview-grid">
                                <div class="overview-stack">
                                    <StatusCard status={st.clone()} hello={capabilities.clone()} busy={pending.refresh} connection={*connection_state} />
                                    <UsbCard
                                        selected_os={(*selected_os).clone()}
                                        on_select_os={{
                                            let selected_os = selected_os.clone();
                                            Callback::from(move |os: String| selected_os.set(os))
                                        }}
                                        usb_enabled={st.as_ref().map(|s| s.usb_enabled).unwrap_or(false)}
                                        connected={usb_control_ready}
                                        starting={pending.usb_start}
                                        stopping={pending.usb_stop}
                                        on_start={on_usb_start.clone()}
                                        on_stop={on_usb_stop.clone()} />
                                </div>
                                <IdentityCard
                                    config={conf.clone()}
                                    usb_enabled={st.as_ref().map(|s| s.usb_enabled).unwrap_or(false)}
                                    connected={device_ready && capabilities.as_ref().is_some_and(|snapshot| snapshot.supports_feature("usb_identity"))}
                                    busy={pending.save_identity}
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
                                connected={script_ready}
                                busy={pending.run_script}
                                supported_layouts={supported_layouts.clone()}
                                on_run={on_run_dsl.clone()} />
                        },
                        AppSection::Transfers => html! {
                            <div class="transfer-workspace">
                                <SecureSessionCard
                                    connected={transfer_capable}
                                    state={*secure_session_view}
                                    on_request={on_request_session.clone()} />
                                <TransferStartCard
                                    path={(*transfer_path).clone()}
                                    connected={transfer_ready}
                                    starting={pending.transfer_start}
                                    setting_default={pending.transfer_default}
                                    on_change={{
                                        let transfer_path = transfer_path.clone();
                                        Callback::from(move |value: String| transfer_path.set(value))
                                    }}
                                    on_start={on_start_transfer.clone()}
                                    on_start_default={on_start_default_transfer.clone()}
                                    on_set_default={on_set_transfer_default.clone()} />
                                <FileBrowserCard
                                    view={(*filesystem_view).clone()}
                                    connected={filesystem_ready}
                                    transfer_enabled={transfer_ready}
                                    transfer_pending={pending.transfer_start}
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

fn monotonic_now_ms() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map(|performance| performance.now())
        .unwrap_or(0.0)
}
