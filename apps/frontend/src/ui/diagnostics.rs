use super::super::*;

#[derive(Properties, PartialEq, Clone)]
pub(crate) struct LogProps {
    pub lines: Vec<String>,
    pub connection: ConnectionState,
    pub on_clear: Callback<()>,
}
#[function_component(LogCard)]
pub(crate) fn log_card(props: &LogProps) -> Html {
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
