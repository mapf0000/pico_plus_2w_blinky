use super::super::*;

#[derive(Properties, PartialEq, Clone)]
pub(crate) struct ScriptProps {
    pub dsl_text: String,
    pub on_change: Callback<String>,
    pub on_select_layout: Callback<String>,
    pub scripts: Option<Vec<api::ScriptMeta>>,
    pub connected: bool,
    pub busy: bool,
    pub supported_layouts: Vec<String>,
    pub on_run: Callback<()>,
}
#[function_component(ScriptingCard)]
pub(crate) fn scripting_card(props: &ScriptProps) -> Html {
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

    let layouts = dsl_core::available_layouts()
        .iter()
        .copied()
        .filter(|layout| {
            props
                .supported_layouts
                .iter()
                .any(|supported| supported == layout)
        })
        .collect::<Vec<_>>();
    let selected_layout = dsl::entry_layout(&props.dsl_text).unwrap_or("");
    let line_count = props.dsl_text.lines().count();
    let can_run = props.connected
        && !props.busy
        && !selected_layout.is_empty()
        && props
            .supported_layouts
            .iter()
            .any(|layout| layout == selected_layout)
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
            <span class="form-note">{if !props.connected { "Script execution is unavailable for this firmware" } else if !props.supported_layouts.iter().any(|layout| layout == selected_layout) { "Select a layout supported by the firmware" } else { "Press Ctrl/⌘ + Enter to run" }}</span>
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
