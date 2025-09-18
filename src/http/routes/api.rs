use core::fmt::Write as _;

use heapless::String;
use picoserve::response::{Response, StatusCode};

use crate::device_config;
use crate::hid::{HID_CHAN, HidCommand, USB_READY};
use crate::host::{self, HostOs};
use crate::scripts;
use crate::{USB_ENABLED, USB_START};

use crate::http::util::{BytesWithType, JsonString, escape_json_str, percent_decode_str};

/// Max payload accepted by /kb/script
const MAX_BODY_BYTES: usize = 512;

// ===== /status =====
pub(crate) async fn route_status() -> impl picoserve::response::IntoResponse {
    let enabled = USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst);
    let ready = USB_READY.load(core::sync::atomic::Ordering::SeqCst);
    log::info!("http: GET /status -> enabled={}, ready={}", enabled, ready);
    let mut body: String<96> = String::new();
    let _ = write!(
        &mut body,
        "{{\"usb_enabled\":{},\"usb_ready\":{},\"host_os\":\"{}\"}}\n",
        enabled,
        ready,
        host::host_os_str()
    );
    Response::ok(JsonString(body))
}

// ===== /config (GET/POST) =====

pub(crate) async fn route_config_get() -> impl picoserve::response::IntoResponse {
    log::info!("http: GET /config");
    let cfg = device_config::get().await;
    let man = escape_json_str(cfg.usb_manufacturer.as_str());
    let prod = escape_json_str(cfg.usb_product.as_str());
    let mut body: String<256> = String::new();
    let _ = write!(
        &mut body,
        "{{\"usb_manufacturer\":\"{}\",\"usb_product\":\"{}\"}}\n",
        man.as_str(),
        prod.as_str()
    );
    Response::ok(JsonString(body))
}

pub(crate) struct ConfigPostService;

impl picoserve::routing::RequestHandlerService<()> for ConfigPostService {
    async fn call_request_handler_service<
        R: picoserve::io::Read,
        W: picoserve::response::ResponseWriter<Error = R::Error>,
    >(
        &self,
        _state: &(),
        _path_parameters: (),
        request: picoserve::request::Request<'_, R>,
        response_writer: W,
    ) -> Result<picoserve::ResponseSent, W::Error> {
        use picoserve::response::IntoResponse;
        log::info!("http: POST /config");
        let query_opt = request.parts.query();
        if USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst) {
            log::warn!("http: /config change rejected (USB already enabled)");
            return (StatusCode::CONFLICT, "USB already enabled\n")
                .write_to(request.body_connection.finalize().await?, response_writer)
                .await;
        }

        let (mut man_dec, mut prod_dec): (
            Option<heapless::String<{ device_config::MANUFACTURER_MAX }>>,
            Option<heapless::String<{ device_config::PRODUCT_MAX }>>,
        ) = (None, None);

        if let Some(q) = query_opt {
            log::debug!("http: /config query='{}'", q.0);
            for pair in q.0.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    if k == "manufacturer" {
                        if let Some(s) =
                            percent_decode_str::<{ device_config::MANUFACTURER_MAX }>(v)
                        {
                            man_dec = Some(s);
                        } else {
                            log::warn!("http: /config bad manufacturer value");
                            return (StatusCode::BAD_REQUEST, "bad manufacturer\n")
                                .write_to(
                                    request.body_connection.finalize().await?,
                                    response_writer,
                                )
                                .await;
                        }
                    } else if k == "product" {
                        if let Some(s) = percent_decode_str::<{ device_config::PRODUCT_MAX }>(v) {
                            prod_dec = Some(s);
                        } else {
                            log::warn!("http: /config bad product value");
                            return (StatusCode::BAD_REQUEST, "bad product\n")
                                .write_to(
                                    request.body_connection.finalize().await?,
                                    response_writer,
                                )
                                .await;
                        }
                    }
                }
            }
        }

        match device_config::set_partial(man_dec.as_deref(), prod_dec.as_deref()).await {
            Ok(()) => {
                let _ = device_config::save().await;
                log::info!(
                    "http: /config updated (manufacturer set? {}, product set? {})",
                    man_dec.is_some(),
                    prod_dec.is_some()
                );
                (StatusCode::OK, "ok\n")
                    .write_to(request.body_connection.finalize().await?, response_writer)
                    .await
            }
            Err(
                device_config::SetError::TooLongManufacturer
                | device_config::SetError::TooLongProduct,
            ) => {
                log::warn!("http: /config value too long");
                (StatusCode::PAYLOAD_TOO_LARGE, "value too long\n")
                    .write_to(request.body_connection.finalize().await?, response_writer)
                    .await
            }
            Err(device_config::SetError::InvalidChars) => {
                log::warn!("http: /config invalid characters");
                (StatusCode::BAD_REQUEST, "invalid characters\n")
                    .write_to(request.body_connection.finalize().await?, response_writer)
                    .await
            }
        }
    }
}

// ===== /usb/register (GET/POST) =====

pub(crate) struct UsbRegisterPostService;

impl picoserve::routing::RequestHandlerService<()> for UsbRegisterPostService {
    async fn call_request_handler_service<
        R: picoserve::io::Read,
        W: picoserve::response::ResponseWriter<Error = R::Error>,
    >(
        &self,
        _state: &(),
        _path_parameters: (),
        request: picoserve::request::Request<'_, R>,
        response_writer: W,
    ) -> Result<picoserve::ResponseSent, W::Error> {
        use picoserve::response::IntoResponse;

        let resp = if USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst) {
            log::info!("http: /usb/register -> already enabled");
            (StatusCode::CONFLICT, "USB already enabled\n")
        } else {
            let mut run_assistant = true;
            let mut host_os: Option<HostOs> = None;
            if let Some(q) = request.parts.query() {
                log::debug!("http: /usb/register query='{}'", q.0);
                for pair in q.0.split('&') {
                    if pair.is_empty() {
                        continue;
                    }
                    if let Some((k, v)) = pair.split_once('=') {
                        if k == "assistant" {
                            if v.eq_ignore_ascii_case("true") || v == "1" {
                                run_assistant = true;
                            } else if v.eq_ignore_ascii_case("false") || v == "0" {
                                run_assistant = false;
                            }
                        } else if k == "os" {
                            if v.eq_ignore_ascii_case("mac") {
                                host_os = Some(HostOs::Mac);
                            } else if v.eq_ignore_ascii_case("windows") {
                                host_os = Some(HostOs::Windows);
                            }
                        }
                    } else if pair == "assistant" {
                        run_assistant = true;
                    }
                }
            }

            if let Some(os) = host_os {
                host::set_host_os(os);
            }
            USB_START.signal(run_assistant);
            log::info!(
                "http: /usb/register signaled (assistant={}, host_os={})",
                run_assistant,
                host::host_os_str()
            );
            if run_assistant {
                (StatusCode::OK, "USB enabling (assistant)\n")
            } else {
                (StatusCode::OK, "USB enabling (no assistant)\n")
            }
        };

        resp.write_to(request.body_connection.finalize().await?, response_writer)
            .await
    }
}

// GET variant for clients that send POST without Content-Length.
pub(crate) async fn route_usb_register_get() -> impl picoserve::response::IntoResponse {
    if USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst) {
        log::info!("http: GET /usb/register -> already enabled");
        return Response::ok(BytesWithType {
            ty: "text/plain; charset=utf-8",
            data: b"USB already enabled\n",
        });
    }

    let run_assistant = true;
    let host_os: Option<HostOs> = None;

    if let Some(os) = host_os {
        host::set_host_os(os);
    }
    USB_START.signal(run_assistant);
    log::info!(
        "http: GET /usb/register signaled (assistant={}, host_os={})",
        run_assistant,
        host::host_os_str()
    );
    let msg = if run_assistant {
        b"USB enabling (assistant)\n".as_slice()
    } else {
        b"USB enabling (no assistant)\n".as_slice()
    };
    Response::ok(BytesWithType { ty: "text/plain; charset=utf-8", data: msg })
}

// ===== /kb/scripts (GET) and /kb/script (POST) =====

pub(crate) struct KbScriptPostService;

impl picoserve::routing::RequestHandlerService<()> for KbScriptPostService {
    async fn call_request_handler_service<
        R: picoserve::io::Read,
        W: picoserve::response::ResponseWriter<Error = R::Error>,
    >(
        &self,
        _state: &(),
        _path_parameters: (),
        mut request: picoserve::request::Request<'_, R>,
        response_writer: W,
    ) -> Result<picoserve::ResponseSent, W::Error> {
        use picoserve::response::IntoResponse;
        log::info!("http: POST /kb/script");
        if !USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst)
            || !USB_READY.load(core::sync::atomic::Ordering::SeqCst)
        {
            log::warn!("http: /kb/script called but USB not ready");
            return (StatusCode::CONFLICT, "USB not ready\n")
                .write_to(request.body_connection.finalize().await?, response_writer)
                .await;
        }

        let body = match request.body_connection.body().read_all().await {
            Ok(b) => b,
            Err(_) => {
                log::warn!("http: /kb/script bad body read");
                return (StatusCode::BAD_REQUEST, "bad body\n")
                    .write_to(request.body_connection.finalize().await?, response_writer)
                    .await;
            }
        };
        if body.is_empty() {
            log::warn!("http: /kb/script empty body");
            return (StatusCode::BAD_REQUEST, "empty body\n")
                .write_to(request.body_connection.finalize().await?, response_writer)
                .await;
        }
        if body.len() > MAX_BODY_BYTES {
            log::warn!("http: /kb/script payload too large ({} bytes)", body.len());
            return (StatusCode::PAYLOAD_TOO_LARGE, "payload too large\n")
                .write_to(request.body_connection.finalize().await?, response_writer)
                .await;
        }
        if !body.iter().all(|b| matches!(b, 9 | 10 | 13 | 32..=126)) {
            log::warn!("http: /kb/script invalid characters in payload");
            return (StatusCode::BAD_REQUEST, "invalid characters\n")
                .write_to(request.body_connection.finalize().await?, response_writer)
                .await;
        }

        let mut s: heapless::String<512> = heapless::String::new();
        for &ch in (&*body).iter() {
            if s.push(ch as char).is_err() {
                log::warn!("http: /kb/script payload too large while copying");
                return (StatusCode::PAYLOAD_TOO_LARGE, "payload too large\n")
                    .write_to(request.body_connection.finalize().await?, response_writer)
                    .await;
            }
        }
        let resp = match HID_CHAN.try_send(HidCommand::RunDsl { dsl: s }) {
            Ok(()) => {
                log::info!("http: /kb/script queued");
                (StatusCode::ACCEPTED, "queued\n")
            }
            Err(_) => {
                log::warn!("http: /kb/script busy");
                (StatusCode::CONFLICT, "busy\n")
            }
        };
        resp.write_to(request.body_connection.finalize().await?, response_writer)
            .await
    }
}

pub(crate) async fn route_kb_scripts_list() -> impl picoserve::response::IntoResponse {
    use picoserve::response::chunked::{ChunkWriter, ChunkedResponse, Chunks};

    struct ScriptsChunks;
    impl Chunks for ScriptsChunks {
        fn content_type(&self) -> &'static str { "application/json" }
        async fn write_chunks<W: picoserve::io::Write>(
            self,
            mut w: ChunkWriter<W>,
        ) -> Result<picoserve::response::chunked::ChunksWritten, W::Error> {
            log::info!("http: GET /kb/scripts ({} entries)", scripts::SCRIPTS.len());
            w.write_chunk(b"[").await?;
            for (i, s) in scripts::SCRIPTS.iter().enumerate() {
                if i != 0 { w.write_chunk(b",").await?; }
                let _ = w
                    .write_fmt(format_args!(
                        "{{\"id\":\"{}\",\"name\":\"{}\",\"description\":\"{}\"",
                        escape_json_str(s.id),
                        escape_json_str(s.name),
                        escape_json_str(s.description)
                    ))
                    .await;
                if let Some(pre) = s.dsl_preview {
                    let _ = w
                        .write_fmt(format_args!(",\"dsl\":\"{}\"", escape_json_str(pre)))
                        .await;
                }
                w.write_chunk(b"}").await?;
            }
            w.write_chunk(b"]\n").await?;
            w.finalize().await
        }
    }

    ChunkedResponse::new(ScriptsChunks)
}
