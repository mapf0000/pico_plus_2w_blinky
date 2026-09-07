use super::super::*;

#[derive(Properties, PartialEq, Clone)]
pub(crate) struct ScriptProps {
    pub python_text: String,
    pub on_change: Callback<String>,
    pub connected: bool,
    pub status: python::ProcessSnapshot,
    pub on_start: Callback<()>,
    pub on_stop: Callback<()>,
}
#[function_component(ScriptingCard)]
pub(crate) fn scripting_card(props: &ScriptProps) -> Html {
    let on_text = {
        let cb = props.on_change.clone();
        Callback::from(move |e: InputEvent| {
            cb.emit(
                e.target_unchecked_into::<web_sys::HtmlTextAreaElement>()
                    .value(),
            )
        })
    };

    let line_count = props.python_text.lines().count();
    let running = matches!(
        props.status.state,
        python::ProcessState::Starting
            | python::ProcessState::Running
            | python::ProcessState::Waiting
    );
    let can_start = !running && line_count <= 1_024 && props.python_text.len() <= 32 * 1024;

    let on_keydown = {
        let cb = props.on_start.clone();
        Callback::from(move |e: KeyboardEvent| {
            if can_start && (e.ctrl_key() || e.meta_key()) && e.key() == "Enter" {
                e.prevent_default();
                cb.emit(());
            }
        })
    };

    html! {
      <div class="script-workspace">
        <section class="card editor-card">
          <div class="card-header editor-header">
            <div><span class="eyebrow">{"RustPython 0.5"}</span><h2>{"Cooperative Python process"}</h2></div>
            <div class="editor-meta">
              <span class={classes!("line-count", (line_count > 1_024).then_some("error"))}>{format!("{line_count} / 1024 lines")}</span>
            </div>
          </div>
          <textarea id="scriptPython" spellcheck="false" aria-label="Python automation process" value={props.python_text.clone()} oninput={on_text} onkeydown={on_keydown} disabled={running} />
          <div class="command-reference">
            <span>{"Yield effects"}</span>
            <code>{"yield tap(\"KEY\")"}</code><code>{"yield text(\"STRING\", 10)"}</code><code>{"yield sleep(MS)"}</code><code>{"yield wait_event(...)"}</code>
          </div>
          <div class="card-actions editor-actions">
            <span class="form-note">{format!("{} — {}{}", match props.status.state { python::ProcessState::Stopped => "Stopped", python::ProcessState::Starting => "Starting", python::ProcessState::Running => "Running", python::ProcessState::Waiting => "Waiting", python::ProcessState::Faulted => "Faulted" }, props.status.message, if props.connected { "" } else { " (device disconnected; process remains alive)" })}</span>
            if running {
              <button id="btnStopPython" class="btn-secondary" onclick={{ let cb=props.on_stop.clone(); Callback::from(move |_| cb.emit(())) }}>{"Stop"}</button>
            } else {
              <button id="btnStartPython" class="btn-primary run-button" disabled={!can_start} onclick={{ let cb=props.on_start.clone(); Callback::from(move |_| cb.emit(())) }}>
                <span aria-hidden="true">{"▶"}</span>{"Start process"}
              </button>
            }
          </div>
        </section>
        <aside class="card library-card">
          <div class="card-header"><div><span class="eyebrow">{"Lifecycle"}</span><h2>{"Browser-owned process"}</h2></div></div>
          <div class="inline-notice">{"The process survives Pico WebSocket disconnects. Closing or reloading this tab stops it. Device effects are never retried automatically."}</div>
        </aside>
      </div>
    }
}
