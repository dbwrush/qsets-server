pub mod models;
pub mod routes;
pub mod services;
pub mod state;

use axum::{
    extract::{Request, State},
    http::{header, HeaderName, HeaderValue},
    middleware::{self, Next},
    response::Response,
    routing::get_service,
    Router,
};
use axum_extra::extract::cookie::CookieJar;
use tower::ServiceBuilder;
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};

use crate::{
    routes::{accounts, api, pages},
    services::auth,
    state::AppState,
};

/// Pages keep all script and style in `/static`, so nothing inline needs to be allowed.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; \
     style-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'self'; \
     form-action 'self'; frame-ancestors 'none'";

fn security_header(name: HeaderName, value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
}

/// Keeps a session alive while it is used: renews it on activity and sends the browser a cookie
/// that lasts as long. Skipped for static files, and when the handler sets the session cookie
/// itself (login, logout).
async fn slide_session(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let renewed = if request.uri().path().starts_with("/static/") {
        None
    } else {
        let jar = CookieJar::from_headers(request.headers());
        match auth::session_token(&jar) {
            Some(token) => auth::renew_session(&state.pool, token)
                .await
                .map(|expires_at| (token, expires_at)),
            None => None,
        }
    };

    let mut response = next.run(request).await;
    let Some((token, expires_at)) = renewed else {
        return response;
    };
    let sets_session = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .any(|value| {
            value
                .to_str()
                .is_ok_and(|v| v.starts_with(&format!("{}=", auth::SESSION_COOKIE)))
        });
    if !sets_session {
        let jar = auth::add_session_cookie(CookieJar::new(), token.to_string(), expires_at);
        for cookie in jar.iter() {
            if let Ok(value) = HeaderValue::from_str(&cookie.to_string()) {
                response.headers_mut().append(header::SET_COOKIE, value);
            }
        }
    }
    response
}

/// Static files are revalidated on every use rather than cached for a fixed time. `ServeDir`
/// sends `Last-Modified` and answers conditional requests with an empty 304, so this stays fast,
/// while an edit reaches browsers and Cloudflare straight away instead of after a default TTL
/// (Cloudflare otherwise keeps `.js`/`.css` for hours).
fn static_files(static_dir: &str) -> Router {
    Router::new()
        .fallback_service(get_service(ServeDir::new(static_dir)))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ))
}

/// The service worker lives in `/static` but is served from the root: a worker can only control
/// the paths at or below its own URL. It must never be cached, or a fixed worker would not reach
/// installed copies until the old one expired.
fn service_worker(static_dir: &str) -> Router {
    let file = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("service-worker-allowed"),
            HeaderValue::from_static("/"),
        ))
        .service(ServeFile::new(format!("{static_dir}/sw.js")));
    Router::new().route_service("/sw.js", file)
}

/// Builds the full application router. `static_dir` is resolved relative to the working
/// directory unless it is absolute.
pub fn app(state: AppState, static_dir: &str) -> Router {
    Router::new()
        .merge(api::router(state.clone()))
        .merge(accounts::router(state.clone()))
        .merge(pages::router(state.clone()))
        .merge(service_worker(static_dir))
        .nest("/static", static_files(static_dir))
        .layer(middleware::from_fn_with_state(state, slide_session))
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
