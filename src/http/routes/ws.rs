use picoserve::response::ws;
use picoserve::io::embedded_io_async; // for Read/Write trait bounds

use crate::host::{self, HostOs};
use crate::usb::hid::{HID_CHAN, HidCommand, USB_READY};
use crate::usb::usb_supervisor;
use crate::http::util::{escape_json_str, percent_decode_str};

pub(crate) async fn ws_handler(
    upgrade: ws::WebSocketUpgrade,
) -> impl picoserve::response::IntoResponse {
    upgrade.on_upgrade(HelloWs)
}

struct HelloWs;

impl ws::WebSocketCallback for HelloWs {
    async fn run<R: embedded_io_async::Read, W: embedded_io_async::Write<Error = R::Error>>(
        self,
        mut rx: ws::SocketRx<R>,
        mut tx: ws::SocketTx<W>,
    ) -> Result<(), W::Error> {
        // greet once
        tx.send_text("hello").await?;

        let mut buf = [0u8; 1024];
        loop {
            match rx.next_message(&mut buf).await {
                Ok(ws::Message::Text(s)) => {
                    let cmd = s.trim();
                    if cmd.is_empty() { continue; }
                    if let Err(_) = handle_command(cmd, &mut tx).await { break; }
                }
                Ok(ws::Message::Binary(_b)) => {
                    // No binary protocol; ignore
                }
                Ok(ws::Message::Ping(p)) => tx.send_pong(p).await?,
                Ok(ws::Message::Pong(_)) => { /* ignore */ }
                Ok(ws::Message::Close(_)) => break,
                Err(_) => break,
            }
        }

        tx.close(None).await
    }
}

// ---- WS command handling ----

/// Handle a single text command and send a text response.
async fn handle_command<W: embedded_io_async::Write>(
    cmd: &str,
    tx: &mut ws::SocketTx<W>,
) -> Result<(), W::Error> {
    if cmd.eq_ignore_ascii_case("STATUS") {
        let enabled = usb_supervisor::USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst);
        let ready = USB_READY.load(core::sync::atomic::Ordering::SeqCst);
        let mut body: heapless::String<96> = heapless::String::new();
        let _ = core::fmt::write(
            &mut body,
            format_args!(
                "{{\"usb_enabled\":{},\"usb_ready\":{},\"host_os\":\"{}\"}}",
                enabled,
                ready,
                host::host_os_str()
            ),
        );
        return tx.send_text(&body).await;
    }

    if cmd.eq_ignore_ascii_case("CONFIG_GET") {
        let cfg = crate::device_config::get().await;
        let man = escape_json_str(cfg.usb_manufacturer.as_str());
        let prod = escape_json_str(cfg.usb_product.as_str());
        let mut body: heapless::String<256> = heapless::String::new();
        let _ = core::fmt::write(
            &mut body,
            format_args!(
                "{{\"usb_manufacturer\":\"{}\",\"usb_product\":\"{}\"}}",
                man.as_str(),
                prod.as_str()
            ),
        );
        return tx.send_text(&body).await;
    }

    if let Some(rest) = cmd.strip_prefix("CONFIG_SET ") {
        // Parse query string: manufacturer=..&product=..
        let mut man_dec: Option<heapless::String<{ crate::device_config::MANUFACTURER_MAX }>> = None;
        let mut prod_dec: Option<heapless::String<{ crate::device_config::PRODUCT_MAX }>> = None;
        for pair in rest.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == "manufacturer" {
                    man_dec = percent_decode_str::<{ crate::device_config::MANUFACTURER_MAX }>(v);
                } else if k == "product" {
                    prod_dec = percent_decode_str::<{ crate::device_config::PRODUCT_MAX }>(v);
                }
            }
        }
        if usb_supervisor::USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst) {
            return tx.send_text("{\"error\":\"USB already enabled\"}").await;
        }
        match crate::device_config::set_partial(man_dec.as_deref(), prod_dec.as_deref()).await {
            Ok(()) => { let _ = crate::device_config::save().await; tx.send_text("{\"ok\":true}").await }
            Err(crate::device_config::SetError::TooLongManufacturer) => tx.send_text("{\"error\":\"bad manufacturer\"}").await,
            Err(crate::device_config::SetError::TooLongProduct) => tx.send_text("{\"error\":\"bad product\"}").await,
            Err(crate::device_config::SetError::InvalidChars) => tx.send_text("{\"error\":\"invalid characters\"}").await,
        }
    } else if cmd.eq_ignore_ascii_case("USB_REGISTER") || cmd.starts_with("USB_REGISTER ") {
        // optional: USB_REGISTER assistant=1&os=mac (we accept but assistant currently unused)
        if let Some(q) = cmd.strip_prefix("USB_REGISTER ") {
            for pair in q.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    if k == "os" {
                        match v {
                            "mac" => host::set_host_os(HostOs::Mac),
                            "windows" => host::set_host_os(HostOs::Windows),
                            _ => host::set_host_os(HostOs::Unknown),
                        }
                    }
                }
            }
        }
        let run_assistant = true;
        let _ = usb_supervisor::start(run_assistant).await;
        return tx.send_text("{\"ok\":true}").await;
    } else if cmd.eq_ignore_ascii_case("USB_UNREGISTER") {
        USB_READY.store(false, core::sync::atomic::Ordering::SeqCst);
        let _ = usb_supervisor::stop(150).await;
        return tx.send_text("{\"ok\":true}").await;
    } else if cmd.eq_ignore_ascii_case("SCRIPTS_LIST") {
        // Build JSON array into a heapless string
        let mut out: heapless::String<2048> = heapless::String::new();
        let _ = out.push('[');
        for (i, s) in crate::scripts::SCRIPTS.iter().enumerate() {
            if i != 0 { let _ = out.push(','); }
            let _ = core::fmt::write(
                &mut out,
                format_args!(
                    "{{\"id\":\"{}\",\"name\":\"{}\",\"description\":\"{}\"",
                    escape_json_str(s.id),
                    escape_json_str(s.name),
                    escape_json_str(s.description)
                ),
            );
            if let Some(pre) = s.dsl_preview {
                let _ = core::fmt::write(
                    &mut out,
                    format_args!(",\"dsl\":\"{}\"", escape_json_str(pre)),
                );
            }
            let _ = out.push('}');
        }
        let _ = out.push_str("]");
        return tx.send_text(&out).await;
    } else if let Some(dsl) = cmd.strip_prefix("SCRIPT_RUN ") {
        let bytes = dsl.as_bytes();
        if bytes.is_empty() || bytes.len() > 512 || !bytes.iter().all(|b| matches!(b, 9 | 10 | 13 | 32..=126)) {
            return tx.send_text("{\"error\":\"bad dsl\"}").await;
        }
        let mut s: heapless::String<512> = heapless::String::new();
        for &ch in bytes.iter() { if s.push(ch as char).is_err() { return tx.send_text("{\"error\":\"dsl too large\"}").await; } }
        match HID_CHAN.try_send(HidCommand::RunDsl { dsl: s }) {
            Ok(()) => tx.send_text("{\"ok\":true,\"queued\":true}").await,
            Err(_) => tx.send_text("{\"error\":\"busy\"}").await,
        }
    } else {
        // Unknown command
        tx.send_text("{\"error\":\"unknown command\"}").await
    }
}
