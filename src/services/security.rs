use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use axum::http::HeaderMap;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use once_cell::sync::Lazy;
use rand::{distributions::Alphanumeric, Rng};
use tokio::sync::Mutex;

const CSRF_COOKIE: &str = "qsets_csrf";
const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);
const LOGIN_MAX_ATTEMPTS: usize = 10;

static LOGIN_ATTEMPTS: Lazy<Mutex<HashMap<String, Vec<Instant>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
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
}
