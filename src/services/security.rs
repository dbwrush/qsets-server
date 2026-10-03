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
use rand::{distributions::Alphanumeric, Rng};
use std::sync::Mutex;

use crate::state::AppState;

const CSRF_COOKIE: &str = "qsets_csrf";
const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Failures allowed for one username from one client: stops guessing a single password.
const MAX_FAILURES_PER_ACCOUNT_AND_CLIENT: u32 = 10;
/// Failures allowed from one client across every username: stops password spraying.
const MAX_FAILURES_PER_CLIENT: u32 = 50;
/// Failures allowed for one username across every client: slows distributed guessing. Kept
/// well above the other limits because anyone can spend it to lock the account out.
const MAX_FAILURES_PER_ACCOUNT: u32 = 100;
/// Expired windows are swept at most this often rather than on every attempt.
const PRUNE_INTERVAL: Duration = Duration::from_secs(60);

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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ThrottleKey {
    AccountAndClient(String, String),
    Client(String),
    Account(String),
}

impl ThrottleKey {
    fn limit(&self) -> u32 {
        match self {
            Self::AccountAndClient(..) => MAX_FAILURES_PER_ACCOUNT_AND_CLIENT,
            Self::Client(_) => MAX_FAILURES_PER_CLIENT,
            Self::Account(_) => MAX_FAILURES_PER_ACCOUNT,
        }
    }

    /// Every key a login for `username` from `ip` counts against. Usernames are compared
    /// case-insensitively so changing case does not buy extra guesses.
    fn all(username: &str, ip: &str) -> [Self; 3] {
        let username = username.trim().to_lowercase();
        [
            Self::AccountAndClient(username.clone(), ip.to_string()),
            Self::Client(ip.to_string()),
            Self::Account(username),
        ]
    }
}

/// Failures counted in a fixed window that starts at the first failure.
#[derive(Debug, Clone, Copy)]
struct FailureWindow {
    started: Instant,
    count: u32,
}

impl FailureWindow {
    fn expired(&self, now: Instant) -> bool {
        now.duration_since(self.started) > LOGIN_WINDOW
    }
}

#[derive(Debug, Default)]
struct ThrottleState {
    failures: HashMap<ThrottleKey, FailureWindow>,
    last_prune: Option<Instant>,
}

impl ThrottleState {
    fn prune(&mut self, now: Instant) {
        if self
            .last_prune
            .is_some_and(|last| now.duration_since(last) < PRUNE_INTERVAL)
        {
            return;
        }
        self.failures.retain(|_, window| !window.expired(now));
        self.last_prune = Some(now);
    }

    fn failures(&self, key: &ThrottleKey, now: Instant) -> u32 {
        match self.failures.get(key) {
            Some(window) if !window.expired(now) => window.count,
            _ => 0,
        }
    }
}

/// Counts failed password checks per account, per client and per account-and-client pair.
///
/// A blocked client stops adding entries, so one address can create at most
/// [`MAX_FAILURES_PER_CLIENT`] entries per window and memory stays bounded by the number of
/// distinct clients.
#[derive(Debug, Default)]
pub struct LoginThrottle {
    state: Mutex<ThrottleState>,
}

impl LoginThrottle {
    fn lock(&self) -> std::sync::MutexGuard<'_, ThrottleState> {
        // The state is only counters, so a panic mid-update cannot leave it unusable.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether another password check is allowed for `username` from `ip`.
    pub fn allows(&self, username: &str, ip: &str) -> bool {
        let now = Instant::now();
        let mut state = self.lock();
        state.prune(now);
        ThrottleKey::all(username, ip)
            .iter()
            .all(|key| state.failures(key, now) < key.limit())
    }

    pub fn record_failure(&self, username: &str, ip: &str) {
        let now = Instant::now();
        let mut state = self.lock();
        for key in ThrottleKey::all(username, ip) {
            let window = state.failures.entry(key).or_insert(FailureWindow {
                started: now,
                count: 0,
            });
            if window.expired(now) {
                *window = FailureWindow {
                    started: now,
                    count: 0,
                };
            }
            window.count += 1;
        }
    }

    /// Forgets failures for this account from this client after it signs in. The per-client
    /// and per-account counts are kept, so a valid login cannot reset an attack in progress.
    pub fn record_success(&self, username: &str, ip: &str) {
        let [pair, _, _] = ThrottleKey::all(username, ip);
        self.lock().failures.remove(&pair);
    }
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

    #[test]
    fn throttle_blocks_repeated_failures_for_one_account_and_client() {
        let throttle = LoginThrottle::default();
        for _ in 0..MAX_FAILURES_PER_ACCOUNT_AND_CLIENT {
            assert!(throttle.allows("alice", "10.0.0.1"));
            throttle.record_failure("alice", "10.0.0.1");
        }
        assert!(!throttle.allows("alice", "10.0.0.1"));
        assert!(!throttle.allows("ALICE", "10.0.0.1"));
        assert!(throttle.allows("alice", "10.0.0.2"));
        assert!(throttle.allows("bob", "10.0.0.1"));
    }

    #[test]
    fn throttle_blocks_password_spraying_from_one_client() {
        let throttle = LoginThrottle::default();
        for n in 0..MAX_FAILURES_PER_CLIENT {
            throttle.record_failure(&format!("user{n}"), "10.0.0.1");
        }
        assert!(!throttle.allows("someone-new", "10.0.0.1"));
        assert!(throttle.allows("someone-new", "10.0.0.2"));
    }

    #[test]
    fn throttle_blocks_distributed_guessing_of_one_account() {
        let throttle = LoginThrottle::default();
        for n in 0..MAX_FAILURES_PER_ACCOUNT {
            throttle.record_failure("alice", &format!("10.0.{}.{}", n / 256, n % 256));
        }
        assert!(!throttle.allows("alice", "192.0.2.1"));
        assert!(throttle.allows("bob", "192.0.2.1"));
    }

    #[test]
    fn successful_login_clears_only_the_account_and_client_pair() {
        let throttle = LoginThrottle::default();
        for _ in 0..MAX_FAILURES_PER_ACCOUNT_AND_CLIENT {
            throttle.record_failure("alice", "10.0.0.1");
        }
        throttle.record_success("alice", "10.0.0.1");
        assert!(throttle.allows("alice", "10.0.0.1"));
        let state = throttle.lock();
        let now = Instant::now();
        assert_eq!(
            state.failures(&ThrottleKey::Client("10.0.0.1".into()), now),
            MAX_FAILURES_PER_ACCOUNT_AND_CLIENT
        );
    }

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
