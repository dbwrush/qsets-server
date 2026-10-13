//! Self-service account lifecycle: input rules and the one-time email tokens behind email
//! verification and password reset.

use chrono::{Duration, Utc};
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::{PgExecutor, PgPool};

pub const VERIFY_EMAIL: &str = "verify_email";
pub const RESET_PASSWORD: &str = "reset_password";

pub const VERIFY_TTL: Duration = Duration::hours(24);
pub const RESET_TTL: Duration = Duration::hours(1);
/// Signups that never confirm their address are deleted after this long.
pub const UNVERIFIED_ACCOUNT_TTL: Duration = Duration::days(3);

const MAX_EMAIL_LEN: usize = 254;
const MIN_USERNAME_LEN: usize = 3;
const MAX_USERNAME_LEN: usize = 32;

/// Trims and checks an address, returning it as typed (case is kept; comparisons lowercase).
/// Deliberately loose: the verification email is the real test of whether it works.
pub fn validate_email(email: &str) -> Result<String, String> {
    let email = email.trim();
    let invalid = || "Enter a valid email address".to_string();
    if email.len() > MAX_EMAIL_LEN || email.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(invalid());
    }
    let (local, domain) = email.split_once('@').ok_or_else(invalid)?;
    let domain_ok = domain.contains('.')
        && !domain.contains('@')
        && !domain.starts_with(['.', '-'])
        && !domain.ends_with(['.', '-'])
        && !domain.contains("..");
    if local.is_empty() || !domain_ok {
        return Err(invalid());
    }
    Ok(email.to_string())
}

/// Rules for new self-registered usernames. `@` is excluded so a username can never be
/// mistaken for an email address when signing in.
pub fn validate_username(username: &str) -> Result<String, String> {
    let username = username.trim();
    let len = username.chars().count();
    if !(MIN_USERNAME_LEN..=MAX_USERNAME_LEN).contains(&len) {
        return Err(format!(
            "Username must be {MIN_USERNAME_LEN}-{MAX_USERNAME_LEN} characters long"
        ));
    }
    if !username
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err("Username may only contain letters, numbers, '_', '-' and '.'".to_string());
    }
    Ok(username.to_string())
}

fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Creates a token for `user_id`, replacing any earlier one for the same purpose, and returns
/// the secret to put in the link. Only its hash is stored.
pub async fn issue_token(
    pool: &PgPool,
    user_id: i32,
    purpose: &str,
    email: &str,
    ttl: Duration,
) -> Result<String, sqlx::Error> {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();

    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM email_tokens WHERE user_id = $1 AND purpose = $2")
        .bind(user_id)
        .bind(purpose)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO email_tokens (token_hash, user_id, purpose, email, expires_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(hash_token(&token))
    .bind(user_id)
    .bind(purpose)
    .bind(email)
    .bind(Utc::now() + ttl)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(token)
}

/// Spends a token, returning the user and address it was issued for. The delete is the check, so
/// a token works once even if two requests race. Run it inside the transaction that acts on it.
pub async fn consume_token<'e>(
    executor: impl PgExecutor<'e>,
    token: &str,
    purpose: &str,
) -> Result<Option<(i32, String)>, sqlx::Error> {
    sqlx::query_as(
        "DELETE FROM email_tokens
         WHERE token_hash = $1 AND purpose = $2 AND expires_at > NOW()
         RETURNING user_id, email",
    )
    .bind(hash_token(token))
    .bind(purpose)
    .fetch_optional(executor)
    .await
}

/// Removes expired tokens and signups whose address was never confirmed.
pub async fn sweep(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let tokens = sqlx::query("DELETE FROM email_tokens WHERE expires_at <= NOW()")
        .execute(pool)
        .await?
        .rows_affected();
    let accounts = sqlx::query(
        "DELETE FROM users
         WHERE email IS NOT NULL AND email_verified_at IS NULL AND created_at < $1",
    )
    .bind(Utc::now() - UNVERIFIED_ACCOUNT_TTL)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(tokens + accounts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_are_checked_loosely_but_sensibly() {
        assert_eq!(validate_email("  a@b.co ").unwrap(), "a@b.co");
        for bad in [
            "", "a", "a@b", "@b.co", "a@@b.co", "a b@c.co", "a@b..co", "a@.co", "a@b.co-",
        ] {
            assert!(validate_email(bad).is_err(), "{bad}");
        }
        assert!(validate_email(&format!("{}@b.co", "a".repeat(260))).is_err());
    }

    #[test]
    fn usernames_cannot_look_like_emails() {
        assert!(validate_username("alice_1").is_ok());
        assert!(validate_username("a@b.co").is_err());
        assert!(validate_username("ab").is_err());
        assert!(validate_username("has space").is_err());
        assert!(validate_username(&"x".repeat(33)).is_err());
    }
}
