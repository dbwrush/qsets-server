use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::{Duration, Utc};
use sqlx::PgPool;
use std::sync::LazyLock;
use uuid::Uuid;

pub const SESSION_COOKIE: &str = "qsets_session";
pub const MIN_PASSWORD_LEN: usize = 8;

/// Shared password policy for every path that sets a password.
pub fn validate_password(password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(format!(
            "Password must be at least {MIN_PASSWORD_LEN} characters long"
        ));
    }
    Ok(())
}

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

/// Hash checked when a username does not exist, made with the same parameters as real hashes so
/// both paths take as long and response timing does not reveal which usernames exist.
static DUMMY_PASSWORD_HASH: LazyLock<String> =
    LazyLock::new(|| hash_password("qsets-dummy-password").expect("hashing a constant succeeds"));

/// Computes the dummy hash up front so the first unknown-user login is not measurably slower.
pub fn prepare_dummy_hash() {
    LazyLock::force(&DUMMY_PASSWORD_HASH);
}

/// Checks a password off the async workers, since Argon2 is deliberately slow. `hash` is `None`
/// for an unknown user, which still pays for one verification and then fails.
pub async fn verify_password_blocking(password: String, hash: Option<String>) -> bool {
    tokio::task::spawn_blocking(move || match hash {
        Some(hash) => verify_password(&password, &hash),
        None => {
            verify_password(&password, &DUMMY_PASSWORD_HASH);
            false
        }
    })
    .await
    .unwrap_or(false)
}

/// Hashes off the async workers; see [`verify_password_blocking`].
pub async fn hash_password_blocking(password: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|e| format!("password hashing task failed: {e}"))?
}

/// Stores a new password hash and signs the user out of every session except `keep`, in one
/// transaction, so a leaked or shared password stops working immediately.
pub async fn set_password(
    pool: &PgPool,
    user_id: i32,
    password_hash: &str,
    keep: Option<Uuid>,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let updated = sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
        .bind(password_hash)
        .bind(user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if updated > 0 {
        revoke_user_sessions(&mut *tx, user_id, keep).await?;
    }
    tx.commit().await?;
    Ok(updated > 0)
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
        "INSERT INTO users (username, password_hash, is_admin, can_upload_pools, tier_id)
         SELECT $1, $2, true, true, id FROM tiers ORDER BY rank DESC LIMIT 1",
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

pub fn session_token(jar: &CookieJar) -> Option<Uuid> {
    Uuid::parse_str(jar.get(SESSION_COOKIE)?.value()).ok()
}

/// Signs a user out of every session except `keep`, e.g. after their password changes.
/// `keep` lets an admin change their own password without logging themselves out.
pub async fn revoke_user_sessions<'e, E>(
    executor: E,
    user_id: i32,
    keep: Option<Uuid>,
) -> Result<u64, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND ($2::uuid IS NULL OR token <> $2)")
        .bind(user_id)
        .bind(keep)
        .execute(executor)
        .await
        .map(|result| result.rows_affected())
}

pub async fn delete_expired_sessions(pool: &PgPool) -> Result<u64, sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE expires_at <= NOW()")
        .execute(pool)
        .await
        .map(|result| result.rows_affected())
}

pub async fn get_user_id_from_jar(pool: &PgPool, jar: &CookieJar) -> Option<i32> {
    let parsed = session_token(jar)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unknown_users_never_verify() {
        assert!(!verify_password_blocking("qsets-dummy-password".into(), None).await);
        let hash = hash_password("real password").unwrap();
        assert!(verify_password_blocking("real password".into(), Some(hash.clone())).await);
        assert!(!verify_password_blocking("wrong".into(), Some(hash)).await);
    }

    #[test]
    fn password_policy_enforces_minimum_length() {
        assert!(validate_password("").is_err());
        assert!(validate_password("short").is_err());
        assert!(validate_password("eightchr").is_ok());
    }
}
