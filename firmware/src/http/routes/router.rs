//! Router assembly kept separate from `mod.rs` for clarity.
//! We expose a macro that builds the Router to avoid naming picoserve's opaque inner types.

/// Build the application router (macro avoids generic type annotation).
/// Usage: `let app = crate::http::routes::app_router!();`
macro_rules! app_router {
    () => {{
        use picoserve::routing::{Router, get};
        // Use absolute paths so macro expansion works from any call site.
        use $crate::http::routes::{api, frontend, ws};

        Router::new()
            // UI / frontend assets
            .route("/", get(frontend::route_frontend_index))
            .route("/ui", get(frontend::route_frontend_index))
            .route("/ui/app.js", get(frontend::route_frontend_js))
            .route("/ui/app.wasm", get(frontend::route_frontend_wasm))
            .route("/ui/style.css", get(frontend::route_frontend_style))
            // Health-only HTTP endpoint (keep simple HTTP for probes)
            .route("/health", get(api::route_health))
            // WebSocket endpoint for all API functionality
            .route("/ws", get(ws::ws_handler))
    }};
}

// Make the macro visible as `crate::http::routes::app_router!()`
pub(crate) use app_router;
