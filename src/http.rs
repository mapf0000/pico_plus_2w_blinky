use core::fmt::Write as _;

use embassy_net as net;
use embassy_time::Duration;
use heapless::String;

use crate::device_config;
use crate::hid::{HID_CHAN, HidCommand, USB_READY};
use crate::host::{self, HostOs};
use crate::scripts;
use crate::{USB_ENABLED, USB_START};

use picoserve::response::{Content, Response, StatusCode};
use picoserve::routing::{RequestHandlerService, Router, get, post_service};

const SERVER_PORT: u16 = 80;

// === Worker pool size ===
// Tune this to match your StackResources<SOCK> budget.
pub const WEB_TASK_POOL_SIZE: usize = 4;

// Centralized HTTP sizes and limits for maintainability
#[cfg(feature = "psram")]
const RX_BUF_SIZE: usize = crate::psram_pool::HTTP_RX_SIZE;
#[cfg(feature = "psram")]
const TX_BUF_SIZE: usize = crate::psram_pool::HTTP_TX_SIZE;
#[cfg(not(feature = "psram"))]
const RX_BUF_SIZE: usize = 4096;
#[cfg(not(feature = "psram"))]
const TX_BUF_SIZE: usize = 4096;
const MAX_BODY_BYTES: usize = 512;
// Request parse buffer (headers + small body staging).
const REQ_BUF_SIZE: usize = 4096;

// For non-PSRAM builds, keep HTTP buffers out of the task stack to avoid overflows.
#[cfg(not(feature = "psram"))]
static mut RX_BUFS: [[u8; RX_BUF_SIZE]; WEB_TASK_POOL_SIZE] =
    [[0; RX_BUF_SIZE]; WEB_TASK_POOL_SIZE];
#[cfg(not(feature = "psram"))]
static mut TX_BUFS: [[u8; TX_BUF_SIZE]; WEB_TASK_POOL_SIZE] =
    [[0; TX_BUF_SIZE]; WEB_TASK_POOL_SIZE];
#[cfg(not(feature = "psram"))]
static mut HTTP_REQ_BUFS: [[u8; REQ_BUF_SIZE]; WEB_TASK_POOL_SIZE] =
    [[0; REQ_BUF_SIZE]; WEB_TASK_POOL_SIZE];

// If PSRAM is unavailable at runtime for a worker, fall back to SRAM per-worker.
#[cfg(feature = "psram")]
static mut FALLBACK_RX_BUFS: [[u8; RX_BUF_SIZE]; WEB_TASK_POOL_SIZE] =
    [[0; RX_BUF_SIZE]; WEB_TASK_POOL_SIZE];
#[cfg(feature = "psram")]
static mut FALLBACK_TX_BUFS: [[u8; TX_BUF_SIZE]; WEB_TASK_POOL_SIZE] =
    [[0; TX_BUF_SIZE]; WEB_TASK_POOL_SIZE];
#[cfg(feature = "psram")]
static mut FALLBACK_HTTP_REQ_BUFS: [[u8; REQ_BUF_SIZE]; WEB_TASK_POOL_SIZE] =
    [[0; REQ_BUF_SIZE]; WEB_TASK_POOL_SIZE];

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
    async fn write_content<W: picoserve::io::Write>(self, mut writer: W) -> Result<(), W::Error> {
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
    async fn write_content<W: picoserve::io::Write>(self, mut writer: W) -> Result<(), W::Error> {
        writer.write_all(self.0.as_bytes()).await
    }
}

// ===== Spawner helper (call this from your init) =====

pub fn spawn_http_server_pool(spawner: &embassy_executor::Spawner, stack: net::Stack<'static>) {
    for id in 0..WEB_TASK_POOL_SIZE {
        match spawner.spawn(server_task(id, stack)) {
            Ok(()) => log::info!("http: spawned worker {id} (port {SERVER_PORT})"),
            Err(e) => log::error!("http: spawn worker {id} failed: {:?}", e),
        }
    }
}

// ===== Server task (pooled) =====

// Note: embassy-net Stack is a Copy handle; pass by value to each worker.
// (Docs: Stack “is Copy, so you can pass it by value instead of by reference”.)
#[embassy_executor::task(pool_size = WEB_TASK_POOL_SIZE)]
pub async fn server_task(id: usize, stack: net::Stack<'static>) -> ! {
    log::info!("http[{id}]: listening on port {}", SERVER_PORT);

    // Build router (read-only; fine to construct per worker)
    let app: Router<_> = Router::new()
        // Serve the compiled WebAssembly frontend at root and under /ui
        .route("/", get(route_frontend_index))
        .route("/ui", get(route_frontend_index))
        .route("/ui/app.js", get(route_frontend_js))
        .route("/ui/app.wasm", get(route_frontend_wasm))
        .route("/ui/style.css", get(route_frontend_style))
        .route("/ui/spider.svg", get(route_frontend_spider))
        // Back-compat and robustness
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

    // Timeouts and connection behavior (keep-alive not needed for curl; browsers may benefit)
    let cfg = picoserve::Config::new(picoserve::Timeouts {
        start_read_request: Some(Duration::from_secs(5)),
        persistent_start_read_request: Some(Duration::from_secs(3)),
        read_request: Some(Duration::from_secs(2)),
        write: Some(Duration::from_secs(3)),
    })
    .close_connection_after_response();

    // --- Buffer selection per worker ---

    #[cfg(feature = "psram")]
    // Try to get PSRAM buffers for this worker; otherwise fall back to per-worker SRAM statics.
    let (rx_buf, tx_buf, http_buf): (&mut [u8], &mut [u8], &mut [u8]) =
        match crate::psram_pool::http_buffers() {
            Some((rx, tx)) => {
                // Local HTTP request buffer (small) on stack per worker is acceptable,
                // but we'll use a fallback static to minimize stack usage.
                let http: &mut [u8] = unsafe { &mut FALLBACK_HTTP_REQ_BUFS[id] };
                (rx, tx, http)
            }
            None => {
                log::warn!("http[{id}]: PSRAM buffers unavailable; falling back to SRAM");
                let rx: &mut [u8] = unsafe { &mut FALLBACK_RX_BUFS[id] };
                let tx: &mut [u8] = unsafe { &mut FALLBACK_TX_BUFS[id] };
                let http: &mut [u8] = unsafe { &mut FALLBACK_HTTP_REQ_BUFS[id] };
                (rx, tx, http)
            }
        };

    #[cfg(not(feature = "psram"))]
    let (rx_buf, tx_buf, http_buf): (&mut [u8], &mut [u8], &mut [u8]) =
        unsafe { (&mut RX_BUFS[id], &mut TX_BUFS[id], &mut HTTP_REQ_BUFS[id]) };

    log::info!(
        "http[{id}]: buffers ready (rx={}, tx={}, req={})",
        RX_BUF_SIZE,
        TX_BUF_SIZE,
        REQ_BUF_SIZE
    );

    // Serve forever; if the listener exits (e.g., stack reset), restart.
    loop {
        // NOTE: Different picoserve versions have slightly different `listen_and_serve`
        // signatures. The common embassy pattern is:
        //   listen_and_serve(<tag>, &app, &cfg, stack, SERVER_PORT, rx, tx, http)
        // Use your crate version’s signature here; this matches the user’s original code.
        picoserve::listen_and_serve(
            "http", // log tag (kept for compatibility)
            &app,
            &cfg,
            stack, // pass by value; Stack is Copy
            SERVER_PORT,
            rx_buf,
            tx_buf,
            http_buf,
        )
        .await;
        log::warn!("http[{id}]: listener exited; restarting");
    }
}

// ===== Handlers =====

// ===== Compiled WebAssembly Frontend =====

mod frontend_static {
    // Generated by build.rs into OUT_DIR
    include!(concat!(env!("OUT_DIR"), "/frontend_static.rs"));
}

async fn route_frontend_index() -> impl picoserve::response::IntoResponse {
    log::debug!("http: serve index.html");
    Response::ok(BytesWithType {
        ty: "text/html; charset=utf-8",
        data: frontend_static::INDEX_HTML.as_bytes(),
    })
}

async fn route_frontend_js() -> impl picoserve::response::IntoResponse {
    log::debug!(
        "http: serve app.js ({} bytes)",
        frontend_static::APP_JS.len()
    );
    Response::ok(BytesWithType {
        ty: "application/javascript",
        data: frontend_static::APP_JS,
    })
}

async fn route_frontend_wasm() -> impl picoserve::response::IntoResponse {
    log::debug!(
        "http: serve app.wasm ({} bytes)",
        frontend_static::APP_WASM.len()
    );
    Response::ok(BytesWithType {
        ty: "application/wasm",
        data: frontend_static::APP_WASM,
    })
}

async fn route_frontend_style() -> impl picoserve::response::IntoResponse {
    log::debug!(
        "http: serve style.css ({} bytes)",
        frontend_static::STYLE_CSS.as_bytes().len()
    );
    Response::ok(BytesWithType {
        ty: "text/css; charset=utf-8",
        data: frontend_static::STYLE_CSS.as_bytes(),
    })
}

async fn route_frontend_spider() -> impl picoserve::response::IntoResponse {
    log::debug!("http: serve spider.svg");
    Response::ok(BytesWithType {
        ty: "image/svg+xml; charset=utf-8",
        data: frontend_static::SPIDER_SVG.as_bytes(),
    })
}

async fn route_status() -> impl picoserve::response::IntoResponse {
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

async fn route_config_get() -> impl picoserve::response::IntoResponse {
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
                {
                    log::warn!("http: /config value too long");
                    (StatusCode::PAYLOAD_TOO_LARGE, "value too long\n")
                }
                .write_to(request.body_connection.finalize().await?, response_writer)
                .await
            }
            Err(device_config::SetError::InvalidChars) => {
                {
                    log::warn!("http: /config invalid characters");
                    (StatusCode::BAD_REQUEST, "invalid characters\n")
                }
                .write_to(request.body_connection.finalize().await?, response_writer)
                .await
            }
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

// Provide a GET variant for clients that send POST without a Content-Length.
async fn route_usb_register_get() -> impl picoserve::response::IntoResponse {
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
            log::info!("http: GET /kb/scripts ({} entries)", scripts::SCRIPTS.len());
            w.write_chunk(b"[").await?;
            for (i, s) in scripts::SCRIPTS.iter().enumerate() {
                if i != 0 {
                    w.write_chunk(b",").await?;
                }
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

// ===== utilities (fixed for UTF-8 correctness) =====

fn escape_json_str(s: &str) -> heapless::String<512> {
    let mut out: heapless::String<512> = heapless::String::new();
    for ch in s.chars() {
        match ch {
            '\"' => {
                let _ = out.push_str("\\\"");
            }
            '\\' => {
                let _ = out.push_str("\\\\");
            }
            '\n' => {
                let _ = out.push_str("\\n");
            }
            '\r' => {
                let _ = out.push_str("\\r");
            }
            '\t' => {
                let _ = out.push_str("\\t");
            }
            c if (c as u32) < 0x20 => {
                let _ = core::fmt::write(&mut out, format_args!("\\u{:04X}", c as u32));
            }
            c => {
                let _ = out.push(c);
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
    let mut out_bytes: heapless::Vec<u8, N> = heapless::Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = hex_val(bytes[i + 1])?;
                let lo = hex_val(bytes[i + 2])?;
                let b = (hi << 4) | lo;
                if out_bytes.push(b).is_err() {
                    return None;
                }
                i += 3;
            }
            b'+' => {
                if out_bytes.push(b' ').is_err() {
                    return None;
                }
                i += 1;
            }
            c => {
                if out_bytes.push(c).is_err() {
                    return None;
                }
                i += 1;
            }
        }
    }
    let s = core::str::from_utf8(&out_bytes).ok()?;
    let mut out: heapless::String<N> = heapless::String::new();
    out.push_str(s).ok()?;
    Some(out)
}
