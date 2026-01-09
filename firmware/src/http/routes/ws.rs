use picoserve::futures::Either;
use picoserve::io::embedded_io_async;
use picoserve::response::ws; // for Read/Write trait bounds

use crate::host::{self, HostOs};
use crate::http::util::{escape_json_str, percent_decode_str};
use crate::usb::hid::{HID_CHAN, HidCommand, MAX_BYTECODE, USB_READY};
use crate::usb::usb_supervisor;
use heapless::Vec;

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
            match rx
                .next_message(&mut buf, core::future::pending::<()>())
                .await
            {
                Ok(Either::First(Ok(ws::Message::Text(s)))) => {
                    let cmd = s.trim();
                    if cmd.is_empty() {
                        continue;
                    }
                    if let Err(_) = handle_command(cmd, &mut tx).await {
                        break;
                    }
                }
                Ok(Either::First(Ok(ws::Message::Binary(_b)))) => {
                    // No binary protocol; ignore
                }
                Ok(Either::First(Ok(ws::Message::Ping(p)))) => tx.send_pong(p).await?,
                Ok(Either::First(Ok(ws::Message::Pong(_)))) => { /* ignore */ }
                Ok(Either::First(Ok(ws::Message::Close(_)))) => break,
                Ok(Either::First(Err(_))) => break,
                Ok(Either::Second(_)) => break,
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
        let mut man_dec: Option<heapless::String<{ crate::device_config::MANUFACTURER_MAX }>> =
            None;
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
            Ok(()) => match crate::device_config::save().await {
                Ok(()) => tx.send_text("{\"ok\":true}").await,
                Err(_) => tx.send_text("{\"error\":\"persist failed\"}").await,
            },
            Err(crate::device_config::SetError::TooLongManufacturer) => {
                tx.send_text("{\"error\":\"bad manufacturer\"}").await
            }
            Err(crate::device_config::SetError::TooLongProduct) => {
                tx.send_text("{\"error\":\"bad product\"}").await
            }
            Err(crate::device_config::SetError::InvalidChars) => {
                tx.send_text("{\"error\":\"invalid characters\"}").await
            }
        }
    } else if cmd.eq_ignore_ascii_case("USB_REGISTER") || cmd.starts_with("USB_REGISTER ") {
        // optional: USB_REGISTER assistant=1&os=mac
        let mut run_assistant = false;
        if let Some(q) = cmd.strip_prefix("USB_REGISTER ") {
            for pair in q.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    if k == "os" {
                        match v {
                            "mac" => host::set_host_os(HostOs::Mac),
                            "windows" => host::set_host_os(HostOs::Windows),
                            _ => host::set_host_os(HostOs::Unknown),
                        }
                    } else if k == "assistant" {
                        run_assistant = v != "0";
                    }
                }
            }
        }
        match usb_supervisor::start(run_assistant).await {
            Ok(()) => return tx.send_text("{\"ok\":true}").await,
            Err(_) => return tx.send_text("{\"error\":\"usb start failed\"}").await,
        }
    } else if cmd.eq_ignore_ascii_case("USB_UNREGISTER") {
        USB_READY.store(false, core::sync::atomic::Ordering::SeqCst);
        match usb_supervisor::stop(150).await {
            Ok(()) => return tx.send_text("{\"ok\":true}").await,
            Err(_) => return tx.send_text("{\"error\":\"usb stop failed\"}").await,
        }
    } else if let Some(hex) = cmd.strip_prefix("SCRIPT_RUN_HEX ") {
        match decode_hex(hex.trim()) {
            Ok(program) => match HID_CHAN.try_send(HidCommand::RunBytecode { program }) {
                Ok(()) => tx.send_text("{\"ok\":true,\"queued\":true}").await,
                Err(_) => tx.send_text("{\"error\":\"busy\"}").await,
            },
            Err(_) => tx.send_text("{\"error\":\"bad bytecode\"}").await,
        }
    } else {
        // Unknown command
        tx.send_text("{\"error\":\"unknown command\"}").await
    }
}

fn decode_hex(input: &str) -> Result<Vec<u8, { MAX_BYTECODE }>, ()> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.len() % 2 != 0 {
        return Err(());
    }
    let max_bytes = trimmed.len() / 2;
    if max_bytes > MAX_BYTECODE {
        return Err(());
    }
    let mut out = Vec::<u8, { MAX_BYTECODE }>::new();
    let bytes = trimmed.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        let hi = decode_nibble(bytes[idx])?;
        let lo = decode_nibble(bytes[idx + 1])?;
        out.push((hi << 4) | lo).map_err(|_| ())?;
        idx += 2;
    }
    Ok(out)
}

fn decode_nibble(ch: u8) -> Result<u8, ()> {
    match ch {
        b'0'..=b'9' => Ok(ch - b'0'),
        b'a'..=b'f' => Ok(10 + ch - b'a'),
        b'A'..=b'F' => Ok(10 + ch - b'A'),
        _ => Err(()),
    }
}
