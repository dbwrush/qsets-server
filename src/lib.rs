pub mod models;
pub mod routes;
pub mod services;
pub mod state;

use axum::{
    http::{header, HeaderName, HeaderValue},
    routing::get_service,
    Router,
};
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer};

use crate::{
    routes::{api, pages},
    state::AppState,
};

/// Pages keep all script and style in `/static`, so nothing inline needs to be allowed.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; \
     style-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'self'; \
     form-action 'self'; frame-ancestors 'none'";

fn security_header(name: HeaderName, value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
}

/// Builds the full application router. `static_dir` is resolved relative to the working
/// directory unless it is absolute.
pub fn app(state: AppState, static_dir: &str) -> Router {
    Router::new()
        .merge(api::router(state.clone()))
        .merge(pages::router(state))
        .nest_service("/static", get_service(ServeDir::new(static_dir)))
        .layer(security_header(
            header::CONTENT_SECURITY_POLICY,
            CONTENT_SECURITY_POLICY,
        ))
        .layer(security_header(header::X_FRAME_OPTIONS, "DENY"))
        .layer(security_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(security_header(
            header::REFERRER_POLICY,
            "strict-origin-when-cross-origin",
        ))
}
