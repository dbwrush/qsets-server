use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::{Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

const SESSION_COOKIE: &str = "qsets_session";

pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|p| p.to_string())
        .map_err(|e| format!("password hashing failed: {e}"))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub async fn create_default_admin(
    pool: &PgPool,
    username: &str,
    password: &str,
) -> Result<(), String> {
    let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE username = $1")
        .bind(username)
        .fetch_one(pool)
        .await
        .map_err(|e| format!("failed checking admin: {e}"))?;

    if exists > 0 {
        return Ok(());
    }

    let hash = hash_password(password)?;
    sqlx::query(
        "INSERT INTO users (username, password_hash, is_admin, tier_id)
         SELECT $1, $2, true, id FROM tiers ORDER BY rank ASC LIMIT 1",
    )
    .bind(username)
    .bind(hash)
    .execute(pool)
    .await
    .map_err(|e| format!("failed creating default admin: {e}"))?;

    Ok(())
}

pub async fn create_session(pool: &PgPool, user_id: i32) -> Result<String, String> {
    let token = Uuid::new_v4();
    let expires_at = Utc::now() + Duration::days(14);

    sqlx::query("INSERT INTO sessions (token, user_id, expires_at) VALUES ($1, $2, $3)")
        .bind(token)
        .bind(user_id)
        .bind(expires_at)
        .execute(pool)
        .await
        .map_err(|e| format!("failed creating session: {e}"))?;

    Ok(token.to_string())
}

pub fn add_session_cookie(jar: CookieJar, token: String) -> CookieJar {
    let secure = std::env::var("COOKIE_SECURE")
        .map(|v| v == "true")
        .unwrap_or(false);
    let cookie = Cookie::build((SESSION_COOKIE, token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .build();
    jar.add(cookie)
}

pub fn clear_session_cookie(jar: CookieJar) -> CookieJar {
    jar.remove(Cookie::from(SESSION_COOKIE))
}

pub async fn destroy_session(pool: &PgPool, token: &str) {
    let Ok(parsed) = Uuid::parse_str(token) else {
        return;
    };

    let _ = sqlx::query("DELETE FROM sessions WHERE token = $1")
        .bind(parsed)
        .execute(pool)
        .await;
}

pub async fn get_user_id_from_jar(pool: &PgPool, jar: &CookieJar) -> Option<i32> {
    let token = jar.get(SESSION_COOKIE)?.value().to_string();
    let Ok(parsed) = Uuid::parse_str(&token) else {
        return None;
    };

    sqlx::query_scalar::<_, i32>(
        "SELECT s.user_id
         FROM sessions s
         WHERE s.token = $1 AND s.expires_at > NOW()",
    )
    .bind(parsed)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}
