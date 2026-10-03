use std::{
    collections::HashMap,
    convert::Infallible,
    net::SocketAddr,
    time::{Duration, Instant},
};

use axum::{
    async_trait,
    extract::{ConnectInfo, FromRequestParts},
    http::{request::Parts, HeaderMap},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use once_cell::sync::Lazy;
use rand::{distributions::Alphanumeric, Rng};
use tokio::sync::Mutex;

use crate::state::AppState;

const CSRF_COOKIE: &str = "qsets_csrf";
const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);
const LOGIN_MAX_ATTEMPTS: usize = 10;

static LOGIN_ATTEMPTS: Lazy<Mutex<HashMap<String, Vec<Instant>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Resolves the address used for throttling and audit logs.
///
/// `X-Forwarded-For` is client-controlled unless a trusted proxy overwrites it, so it is only
/// consulted when `trust_proxy` is set. In that case the right-most entry is used: it is the one
/// appended by the proxy directly in front of us, while anything to its left came from the client.
pub fn resolve_client_ip(
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    trust_proxy: bool,
) -> String {
    if trust_proxy {
        let forwarded = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        if let Some(ip) = forwarded {
            return ip.to_string();
        }
    }

    peer.map(|addr| addr.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Request origin details recorded in audit logs and used as the login throttle key.
#[derive(Debug, Clone)]
pub struct ClientMeta {
    pub ip: String,
    pub user_agent: Option<String>,
}

#[async_trait]
impl FromRequestParts<AppState> for ClientMeta {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| *addr);
        Ok(Self {
            ip: resolve_client_ip(&parts.headers, peer, state.trust_proxy),
            user_agent: user_agent(&parts.headers),
        })
    }
}

pub fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string())
}

pub async fn allow_login_attempt(key: &str) -> bool {
    let mut map = LOGIN_ATTEMPTS.lock().await;
    let now = Instant::now();
    map.retain(|_, attempts| {
        attempts.retain(|t| now.duration_since(*t) <= LOGIN_WINDOW);
        !attempts.is_empty()
    });
    let entry = map.entry(key.to_string()).or_default();
    entry.len() < LOGIN_MAX_ATTEMPTS
}

pub async fn record_login_attempt(key: &str) {
    let mut map = LOGIN_ATTEMPTS.lock().await;
    let now = Instant::now();
    let entry = map.entry(key.to_string()).or_default();
    entry.push(now);
    entry.retain(|t| now.duration_since(*t) <= LOGIN_WINDOW);
}

pub async fn clear_login_attempts(key: &str) {
    let mut map = LOGIN_ATTEMPTS.lock().await;
    map.remove(key);
}

pub fn ensure_csrf_cookie(jar: CookieJar) -> (CookieJar, String) {
    if let Some(v) = jar.get(CSRF_COOKIE).map(|c| c.value().to_string()) {
        return (jar, v);
    }

    let token: String = rand::thread_rng()
        .sample_iter(Alphanumeric)
        .take(48)
        .map(char::from)
        .collect();
    let secure = std::env::var("COOKIE_SECURE")
        .map(|v| v == "true")
        .unwrap_or(false);

    let cookie = Cookie::build((CSRF_COOKIE, token.clone()))
        .path("/")
        .http_only(false)
        .same_site(SameSite::Strict)
        .secure(secure)
        .build();

    (jar.add(cookie), token)
}

pub fn verify_csrf(jar: &CookieJar, headers: &HeaderMap) -> bool {
    let a = jar.get(CSRF_COOKIE).map(|c| c.value().to_string());
    let b = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    matches!((a, b), (Some(x), Some(y)) if x == y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csrf_jar(value: &str) -> CookieJar {
        CookieJar::new().add(Cookie::new(CSRF_COOKIE, value.to_string()))
    }

    #[test]
    fn csrf_requires_cookie_and_matching_header() {
        let jar = csrf_jar("token");
        let mut headers = HeaderMap::new();
        headers.insert("x-csrf-token", "token".parse().unwrap());
        assert!(verify_csrf(&jar, &headers));

        headers.insert("x-csrf-token", "wrong".parse().unwrap());
        assert!(!verify_csrf(&jar, &headers));
        assert!(!verify_csrf(&CookieJar::new(), &headers));
    }

    fn forwarded(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", value.parse().unwrap());
        headers
    }

    #[test]
    fn forwarded_header_is_ignored_unless_proxy_is_trusted() {
        let peer: SocketAddr = "10.0.0.5:4000".parse().unwrap();
        let headers = forwarded("203.0.113.9");
        assert_eq!(resolve_client_ip(&headers, Some(peer), false), "10.0.0.5");
        assert_eq!(resolve_client_ip(&headers, None, false), "unknown");
    }

    #[test]
    fn trusted_proxy_uses_right_most_forwarded_entry() {
        let peer: SocketAddr = "127.0.0.1:4000".parse().unwrap();
        let headers = forwarded("1.2.3.4, 203.0.113.9");
        assert_eq!(resolve_client_ip(&headers, Some(peer), true), "203.0.113.9");
        assert_eq!(
            resolve_client_ip(&HeaderMap::new(), Some(peer), true),
            "127.0.0.1"
        );
    }
}
