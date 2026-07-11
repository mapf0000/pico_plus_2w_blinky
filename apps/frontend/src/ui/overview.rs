use super::super::*;

#[derive(Properties, PartialEq, Clone)]
pub(crate) struct StatusProps {
    pub status: Option<StatusState>,
    pub hello: Option<api::Hello>,
    pub busy: bool,
    pub connection: ConnectionState,
}
#[function_component(StatusCard)]
pub(crate) fn status_card(props: &StatusProps) -> Html {
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
                <div class="metric"><span>{"Firmware"}</span><strong>{props.hello.as_ref().map(|hello| hello.firmware.version.as_str()).unwrap_or("Negotiating…")}</strong></div>
                <div class="metric"><span>{"Host agent"}</span><strong>{props.hello.as_ref().map(|hello| if hello.host_agent.present { hello.host_agent.version.as_deref().unwrap_or("Update required") } else { "Not detected" }).unwrap_or("Checking…")}</strong></div>
            </div>
            <div class="card-footer-note"><span class="pulse-dot"></span>{"Status refreshes every five seconds while connected"}</div>
        </section>
    }
}

#[derive(Properties, PartialEq, Clone)]
pub(crate) struct IdentityProps {
    pub config: Option<ConfigState>,
    pub usb_enabled: bool,
    pub connected: bool,
    pub busy: bool,
    pub on_save: Callback<(String, String)>,
}
#[function_component(IdentityCard)]
pub(crate) fn identity_card(props: &IdentityProps) -> Html {
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
pub(crate) struct UsbProps {
    pub selected_os: String,
    pub on_select_os: Callback<String>,
    pub on_start: Callback<bool>,
    pub on_stop: Callback<()>,
    pub usb_enabled: bool,
    pub connected: bool,
    pub starting: bool,
    pub stopping: bool,
}
#[function_component(UsbCard)]
pub(crate) fn usb_card(props: &UsbProps) -> Html {
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
                <button id="btnUsbAssistant" class="btn-primary" onclick={start_assist} disabled={!props.connected || props.starting || props.selected_os=="windows"}>{if props.starting { "Starting…" } else { "Start with Assistant" }}</button>
                <button id="btnUsbNoAssistant" class="btn-secondary" onclick={start_noassist} disabled={!props.connected || props.starting}>{if props.starting { "Starting…" } else { "Start USB" }}</button>
              </div>
              <div class="card-footer-note">{"Assistant runs the macOS keyboard identification sequence after registration."}</div>
            </>
          } else {
            <div class="active-state"><span class="active-state-icon">{"✓"}</span><div><strong>{"USB is active"}</strong><span>{"The device is registered with the host."}</span></div></div>
            <div class="card-actions left">
              <button id="btnUsbStop" class="btn-danger" disabled={!props.connected || props.stopping} onclick={{ let cb = props.on_stop.clone(); Callback::from(move |_| cb.emit(())) }}>{if props.stopping { "Stopping…" } else { "Stop USB" }}</button>
            </div>
          }
        </section>
    }
}
