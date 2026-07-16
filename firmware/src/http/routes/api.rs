use crate::http::util::BytesWithType;
use picoserve::response::Response;

// Keep HTTP surface minimal: only health check (UI and WS are routed elsewhere)
pub(crate) async fn route_health() -> impl picoserve::response::IntoResponse {
    crate::health::mark(crate::health::Stage::HttpHealth);
    log::debug!("http: GET /health");
    Response::ok(BytesWithType {
        ty: "text/plain; charset=utf-8",
        data: b"ok\n",
    })
}
