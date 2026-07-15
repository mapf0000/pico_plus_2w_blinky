use super::super::*;

#[derive(Properties, PartialEq, Clone)]
pub(crate) struct SecureSessionProps {
    pub connected: bool,
    pub state: transfer::SecureSessionView,
    pub on_request: Callback<()>,
}

#[function_component(SecureSessionCard)]
pub(crate) fn secure_session_card(props: &SecureSessionProps) -> Html {
    let on_request = {
        let callback = props.on_request.clone();
        Callback::from(move |_| callback.emit(()))
    };
    let busy = matches!(
        props.state,
        transfer::SecureSessionView::Negotiating | transfer::SecureSessionView::Handshaking
    );

    html! {
        <section class="card secure-session-card">
            <div class="card-header">
                <div><span class="eyebrow">{"File-transfer security"}</span><h2>{"Host encryption session"}</h2></div>
                <span class={classes!("connection-pill", props.state.established().then_some("is-good"))}>{props.state.label()}</span>
            </div>
            <p class="hint">{"The browser connects automatically and encrypts file transfers. Unattended mode trusts every client that can access this Pico Web UI."}</p>
            <div class="transfer-path-row">
                <button class="btn-secondary" disabled={!props.connected || props.state.established()} onclick={on_request}>{if busy { "Retry now" } else { "Reconnect" }}</button>
            </div>
        </section>
    }
}

#[derive(Properties, PartialEq, Clone)]
pub(crate) struct FileBrowserProps {
    pub view: filesystem::BrowserView,
    pub connected: bool,
    pub transfer_enabled: bool,
    pub transfer_pending: bool,
    pub on_browse: Callback<BrowseRequest>,
    pub on_select: Callback<String>,
    pub on_transfer: Callback<String>,
}

#[function_component(FileBrowserCard)]
pub(crate) fn file_browser_card(props: &FileBrowserProps) -> Html {
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
                        <button class="btn-primary btn-small" onclick={transfer} disabled={!entry.readable || !props.transfer_enabled || props.transfer_pending}>{if props.transfer_pending { "Queueing…" } else { "Transfer" }}</button>
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
pub(crate) struct TransferStartProps {
    pub path: String,
    pub connected: bool,
    pub starting: bool,
    pub setting_default: bool,
    pub on_change: Callback<String>,
    pub on_start: Callback<()>,
    pub on_start_default: Callback<()>,
    pub on_set_default: Callback<()>,
}

#[function_component(TransferStartCard)]
pub(crate) fn transfer_start_card(props: &TransferStartProps) -> Html {
    let base_disabled = !props.connected || props.path.trim().is_empty();
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

    let on_start_default = {
        let cb = props.on_start_default.clone();
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
            <button class="btn-primary" disabled={base_disabled || props.starting} onclick={on_start}>{if props.starting { "Queueing…" } else { "Queue transfer" }}</button>
            <button class="btn-secondary" disabled={base_disabled || props.setting_default} onclick={on_set_default}>{if props.setting_default { "Saving…" } else { "Set as default" }}</button>
            <button class="btn-secondary" disabled={!props.connected || props.starting} onclick={on_start_default}>{"Queue default"}</button>
          </div>
          <div class="card-footer-note">{if props.connected { "The path and every file record are authenticated and encrypted before leaving the host-agent process." } else { "Establish an encrypted host session before queueing a transfer." }}</div>
        </section>
    }
}
#[derive(Properties, PartialEq, Clone)]
pub(crate) struct DownloadManagerProps {
    pub transfers: Vec<transfer::TransferView>,
    pub on_download: Callback<u64>,
}

#[function_component(DownloadManagerCard)]
pub(crate) fn download_manager_card(props: &DownloadManagerProps) -> Html {
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
