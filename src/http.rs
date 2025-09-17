use core::fmt::Write as _;

use embassy_net as net;
use heapless::String;

use crate::device_config;
use crate::hid::{HID_CHAN, HidCommand, USB_READY};
use crate::host::{self, HostOs};
use crate::scripts;
use crate::{USB_ENABLED, USB_START};

use picoserve::response::{Content, Response, StatusCode};
use picoserve::routing::{get, post_service, RequestHandlerService, Router};

const SERVER_PORT: u16 = 80;
// Centralized HTTP sizes and limits for maintainability
#[cfg(feature = "psram")]
const RX_BUF_SIZE: usize = crate::psram_pool::HTTP_RX_SIZE;
#[cfg(feature = "psram")]
const TX_BUF_SIZE: usize = crate::psram_pool::HTTP_TX_SIZE;
#[cfg(not(feature = "psram"))]
const RX_BUF_SIZE: usize = 1024;
#[cfg(not(feature = "psram"))]
const TX_BUF_SIZE: usize = 1024;
const MAX_BODY_BYTES: usize = 512;
// Request parse buffer (headers + small body staging); keep modest to avoid stack bloat.
const REQ_BUF_SIZE: usize = 1024;

// Small Content helpers to set precise content types without duplicating headers.
struct BytesWithType<'a> {
    ty: &'static str,
    data: &'a [u8],
}

impl<'a> Content for BytesWithType<'a> {
    fn content_type(&self) -> &'static str {
        self.ty
    }
    fn content_length(&self) -> usize {
        self.data.len()
    }
    async fn write_content<W: picoserve::io::Write>(
        self,
        mut writer: W,
    ) -> Result<(), W::Error> {
        writer.write_all(self.data).await
    }
}

struct JsonString<const N: usize>(heapless::String<N>);
impl<const N: usize> Content for JsonString<N> {
    fn content_type(&self) -> &'static str {
        "application/json"
    }
    fn content_length(&self) -> usize {
        self.0.len()
    }
    async fn write_content<W: picoserve::io::Write>(
        self,
        mut writer: W,
    ) -> Result<(), W::Error> {
        writer.write_all(self.0.as_bytes()).await
    }
}

// (no extra extractors required)

// No extra bodyless extractor needed; handlers that only need parts can add an unused body param like `&_ [u8]` as the last arg.

#[embassy_executor::task]
pub async fn server_task(stack: &'static net::Stack<'static>) -> ! {
    log::info!("http: listening on port {}", SERVER_PORT);

    // Build router
    let app: Router<_> = Router::new()
        // Serve the compiled WebAssembly frontend at root and under /ui
        .route("/", get(route_frontend_index))
        // Alias path and assets
        .route("/ui", get(route_frontend_index))
        .route("/ui/app.js", get(route_frontend_js))
        .route("/ui/app.wasm", get(route_frontend_wasm))
        .route("/ui/style.css", get(route_frontend_style))
        .route("/ui/spider.svg", get(route_frontend_spider))
        // Back-compat and robustness: handle accidental double "/ui/ui/*" and legacy top-level paths
        .route("/ui/ui/style.css", get(route_frontend_style))
        .route("/ui/ui/spider.svg", get(route_frontend_spider))
        .route("/style.css", get(route_frontend_style))
        .route("/spider.svg", get(route_frontend_spider))
        // API endpoints
        .route("/status", get(route_status))
        .route(
            "/config",
            get(route_config_get).post_service(ConfigPostService),
        )
        .route("/kb/scripts", get(route_kb_scripts_list))
        .route("/kb/script", post_service(KbScriptPostService))
        .route(
            "/usb/register",
            get(route_usb_register_get).post_service(UsbRegisterPostService),
        );

    // Timeouts and connection behavior
    let cfg = picoserve::Config::new(picoserve::Timeouts {
        start_read_request: None,
        persistent_start_read_request: None,
        read_request: None,
        write: None,
    })
    .close_connection_after_response();

    // Buffers
    #[cfg(feature = "psram")]
    let (mut rx_ps, mut tx_ps) = match crate::psram_pool::http_buffers() {
        Some((a, b)) => (a, b),
        None => {
            log::warn!("http: PSRAM buffers unavailable; falling back to SRAM");
            static mut FALLBACK_RX: [u8; RX_BUF_SIZE] = [0; RX_BUF_SIZE];
            static mut FALLBACK_TX: [u8; TX_BUF_SIZE] = [0; TX_BUF_SIZE];
            unsafe { (&mut FALLBACK_RX, &mut FALLBACK_TX) }
        }
    };
    let mut http_buf = [0u8; REQ_BUF_SIZE];

    #[cfg(feature = "psram")]
    {
        picoserve::listen_and_serve(
            "http",
            &app,
            &cfg,
            *stack,
            SERVER_PORT,
            &mut rx_ps,
            &mut tx_ps,
            &mut http_buf,
        )
        .await
    }

    #[cfg(not(feature = "psram"))]
    {
        let mut rx_buf = [0u8; RX_BUF_SIZE];
        let mut tx_buf = [0u8; TX_BUF_SIZE];
        picoserve::listen_and_serve(
            "http",
            &app,
            &cfg,
            *stack,
            SERVER_PORT,
            &mut rx_buf,
            &mut tx_buf,
            &mut http_buf,
        )
        .await
    }
}

// ===== Handlers =====

// Legacy root HTML removed; compiled frontend serves at / and /ui

// ===== Compiled WebAssembly Frontend =====

mod frontend_static {
    // Generated by build.rs into OUT_DIR
    include!(concat!(env!("OUT_DIR"), "/frontend_static.rs"));
}

async fn route_frontend_index() -> impl picoserve::response::IntoResponse {
    Response::ok(BytesWithType {
        ty: "text/html; charset=utf-8",
        data: frontend_static::INDEX_HTML.as_bytes(),
    })
}

async fn route_frontend_js() -> impl picoserve::response::IntoResponse {
    Response::ok(BytesWithType {
        ty: "application/javascript",
        data: frontend_static::APP_JS,
    })
}

async fn route_frontend_wasm() -> impl picoserve::response::IntoResponse {
    Response::ok(BytesWithType {
        ty: "application/wasm",
        data: frontend_static::APP_WASM,
    })
}

async fn route_frontend_style() -> impl picoserve::response::IntoResponse {
    Response::ok(BytesWithType {
        ty: "text/css; charset=utf-8",
        data: frontend_static::STYLE_CSS.as_bytes(),
    })
}

async fn route_frontend_spider() -> impl picoserve::response::IntoResponse {
    Response::ok(BytesWithType {
        ty: "image/svg+xml; charset=utf-8",
        data: frontend_static::SPIDER_SVG.as_bytes(),
    })
}

async fn route_status() -> impl picoserve::response::IntoResponse {
    let enabled = USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst);
    let ready = USB_READY.load(core::sync::atomic::Ordering::SeqCst);
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

// Legacy /style.css and /spider.svg removed; assets are under /ui/*.

async fn route_config_get() -> impl picoserve::response::IntoResponse {
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

struct ConfigPostService;

impl RequestHandlerService<()> for ConfigPostService {
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
        let query_opt = request.parts.query();
        if USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst) {
            return (StatusCode::CONFLICT, "USB already enabled\n")
                .write_to(request.body_connection.finalize().await?, response_writer)
                .await;
        }

        let (mut man_dec, mut prod_dec): (
            Option<heapless::String<{ device_config::MANUFACTURER_MAX }>>,
            Option<heapless::String<{ device_config::PRODUCT_MAX }>>,
        ) = (None, None);

        if let Some(q) = query_opt {
            for pair in q.0.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    if k == "manufacturer" {
                        if let Some(s) =
                            percent_decode_str::<{ device_config::MANUFACTURER_MAX }>(v)
                        {
                            man_dec = Some(s);
                        } else {
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
                (StatusCode::OK, "ok\n")
                    .write_to(request.body_connection.finalize().await?, response_writer)
                    .await
            }
            Err(device_config::SetError::TooLongManufacturer
            | device_config::SetError::TooLongProduct) => (StatusCode::PAYLOAD_TOO_LARGE, "value too long\n")
                .write_to(request.body_connection.finalize().await?, response_writer)
                .await,
            Err(device_config::SetError::InvalidChars) => (StatusCode::BAD_REQUEST, "invalid characters\n")
                .write_to(request.body_connection.finalize().await?, response_writer)
                .await,
        }
    }
}

struct UsbRegisterPostService;

impl RequestHandlerService<()> for UsbRegisterPostService {
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
            (StatusCode::CONFLICT, "USB already enabled\n")
        } else {
            let mut run_assistant = true;
            let mut host_os: Option<HostOs> = None;
            if let Some(q) = request.parts.query() {
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
            if run_assistant {
                (StatusCode::OK, "USB enabling (assistant)\n")
            } else {
                (StatusCode::OK, "USB enabling (no assistant)\n")
            }
        };

        resp
            .write_to(request.body_connection.finalize().await?, response_writer)
            .await
    }
}

// Provide a GET variant for clients that send POST without a Content-Length (e.g.,
// some curl invocations), which can cause body finalization to hang. The GET path
// performs the same action using query parameters and returns immediately.
async fn route_usb_register_get() -> impl picoserve::response::IntoResponse {
    if USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst) {
        return Response::ok(BytesWithType {
            ty: "text/plain; charset=utf-8",
            data: b"USB already enabled\n",
        });
    }

    let mut run_assistant = true;
    let mut host_os: Option<HostOs> = None;
    // Access to query string for GET handlers is via picoserve request parts when
    // using services; here we can't access it directly. Parse it from the index
    // HTML’s rewritten paths is not available, so default to assistant=true.
    // For consistency with the POST handler, we allow overriding via a simple
    // environment-independent mechanism: the frontend uses POST; manual users can
    // call GET /usb/register?assistant=0&os=windows. We retrieve the query via
    // the global host module helper that caches the last set OS when POST was
    // used. If not present, we keep defaults.
    // Note: picoserve doesn’t expose request parts in a function handler, so we
    // cannot read the actual query here; stick to defaults.

    if let Some(os) = host_os {
        host::set_host_os(os);
    }
    USB_START.signal(run_assistant);
    let msg = if run_assistant {
        b"USB enabling (assistant)\n".as_slice()
    } else {
        b"USB enabling (no assistant)\n".as_slice()
    };
    Response::ok(BytesWithType {
        ty: "text/plain; charset=utf-8",
        data: msg,
    })
}

struct KbScriptPostService;

impl RequestHandlerService<()> for KbScriptPostService {
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
        if !USB_ENABLED.load(core::sync::atomic::Ordering::SeqCst)
            || !USB_READY.load(core::sync::atomic::Ordering::SeqCst)
        {
            return (StatusCode::CONFLICT, "USB not ready\n")
                .write_to(request.body_connection.finalize().await?, response_writer)
                .await;
        }

        let body = match request.body_connection.body().read_all().await {
            Ok(b) => b,
            Err(_) => {
                return (StatusCode::BAD_REQUEST, "bad body\n")
                    .write_to(request.body_connection.finalize().await?, response_writer)
                    .await;
            }
        };
        if body.is_empty() {
            return (StatusCode::BAD_REQUEST, "empty body\n")
                .write_to(request.body_connection.finalize().await?, response_writer)
                .await;
        }
        if body.len() > MAX_BODY_BYTES {
            return (StatusCode::PAYLOAD_TOO_LARGE, "payload too large\n")
                .write_to(request.body_connection.finalize().await?, response_writer)
                .await;
        }
        if !body
            .iter()
            .all(|b| matches!(b, 9 | 10 | 13 | 32..=126))
        {
            return (StatusCode::BAD_REQUEST, "invalid characters\n")
                .write_to(request.body_connection.finalize().await?, response_writer)
                .await;
        }

        let mut s: heapless::String<512> = heapless::String::new();
        for &ch in (&*body).iter() {
            if s.push(ch as char).is_err() {
                return (StatusCode::PAYLOAD_TOO_LARGE, "payload too large\n")
                    .write_to(request.body_connection.finalize().await?, response_writer)
                    .await;
            }
        }
        let resp = match HID_CHAN.try_send(HidCommand::RunDsl { dsl: s }) {
            Ok(()) => (StatusCode::ACCEPTED, "queued\n"),
            Err(_) => (StatusCode::CONFLICT, "busy\n"),
        };
        resp
            .write_to(request.body_connection.finalize().await?, response_writer)
            .await
    }
}

async fn route_kb_scripts_list() -> impl picoserve::response::IntoResponse {
    use picoserve::response::chunked::{ChunkWriter, ChunkedResponse, Chunks};

    struct ScriptsChunks;
    impl Chunks for ScriptsChunks {
        fn content_type(&self) -> &'static str {
            "application/json"
        }
        async fn write_chunks<W: picoserve::io::Write>(
            self,
            mut w: ChunkWriter<W>,
        ) -> Result<picoserve::response::chunked::ChunksWritten, W::Error> {
            w.write_chunk(b"[").await?;
            for (i, s) in scripts::SCRIPTS.iter().enumerate() {
                if i != 0 {
                    w.write_chunk(b",").await?;
                }
                // Minimal JSON object per entry
                // {"id":"...","name":"...","description":"...","dsl":"..."}
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

// ===== utilities reused from previous implementation =====

fn escape_json_str(s: &str) -> heapless::String<512> {
    let mut out: heapless::String<512> = heapless::String::new();
    for b in s.bytes() {
        match b {
            b'"' => {
                let _ = out.push_str("\\\"");
            }
            b'\\' => {
                let _ = out.push_str("\\\\");
            }
            b'\n' => {
                let _ = out.push_str("\\n");
            }
            b'\r' => {
                let _ = out.push_str("\\r");
            }
            b'\t' => {
                let _ = out.push_str("\\t");
            }
            _ => {
                let _ = out.push(b as char);
            }
        }
    }
    out
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn percent_decode_str<const N: usize>(s: &str) -> Option<heapless::String<N>> {
    let bytes = s.as_bytes();
    let mut out: heapless::String<N> = heapless::String::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = hex_val(bytes[i + 1])?;
                let lo = hex_val(bytes[i + 2])?;
                let ch = (hi << 4) | lo;
                if out.push(ch as char).is_err() {
                    return None;
                }
                i += 3;
            }
            b'+' => {
                if out.push(' ').is_err() {
                    return None;
                }
                i += 1;
            }
            c => {
                if out.push(c as char).is_err() {
                    return None;
                }
                i += 1;
            }
        }
    }
    Some(out)
}
