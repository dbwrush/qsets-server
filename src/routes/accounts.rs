//! Self-service accounts: registration, email verification and password reset.
//!
//! Requests that mail someone answer the same way whether or not the address belongs to an
//! account, and do their lookups and sending in a background task, so neither the response nor
//! its timing reveals which addresses are registered.

use axum::{
    extract::State, http::StatusCode, middleware, response::IntoResponse, routing::post, Json,
    Router,
};
use serde::Deserialize;
use serde_json::json;

use super::common::{
    db_error, is_unique_violation, require_csrf, require_user, ApiError, ApiResult,
};
use crate::{
    services::{
        accounts::{self, RESET_PASSWORD, RESET_TTL, VERIFY_EMAIL, VERIFY_TTL},
        audit, auth,
        authz::Actor,
        mail,
        security::ClientMeta,
    },
    state::AppState,
};

const MAX_BODY_BYTES: usize = 16 * 1024;
const INVALID_LINK: &str = "This link is invalid or has expired";

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/register", post(register))
        .route("/api/email/verify", post(verify_email))
        .route("/api/email/resend", post(resend_verification))
        .route("/api/me/email", post(change_email))
        .route("/api/password-reset/request", post(request_password_reset))
        .route("/api/password-reset/confirm", post(confirm_password_reset))
        .route_layer(middleware::from_fn(require_csrf))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

fn message(text: &str) -> ApiResult {
    Ok(Json(json!({ "message": text })).into_response())
}

/// Counts an email-sending request against the client's and the destination's hourly limits.
fn allow_email(state: &AppState, client: &ClientMeta, address: &str) -> ApiResult<()> {
    if state.email_client_limiter.allow(&client.ip)
        && state.email_address_limiter.allow(&address.to_lowercase())
    {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many requests; try again later",
        ))
    }
}

fn link(state: &AppState, path: &str, token: &str) -> String {
    format!("{}{path}?token={token}", state.public_base_url)
}

fn send_later(state: &AppState, to: String, (subject, body): (&'static str, String)) {
    let mailer = state.mailer.clone();
    tokio::spawn(async move {
        if let Err(error) = mailer.send(&to, subject, &body).await {
            tracing::error!(error = %error, "failed to send email");
        }
    });
}

async fn address_has_verified_owner(state: &AppState, email: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM users
                        WHERE email_verified_at IS NOT NULL AND lower(email) = lower($1))",
    )
    .bind(email)
    .fetch_one(&state.pool)
    .await
}

/// Mails a verification link for `email` to `user_id`, or a notice when someone else already
/// verified that address. Failures are logged: the caller has already answered.
async fn mail_verification(state: &AppState, user_id: i32, email: &str) {
    let result: Result<(), sqlx::Error> = async {
        if address_has_verified_owner(state, email).await? {
            let reset_url = format!("{}/forgot-password", state.public_base_url);
            send_later(
                state,
                email.to_string(),
                mail::address_in_use_message(&reset_url),
            );
            return Ok(());
        }
        let token =
            accounts::issue_token(&state.pool, user_id, VERIFY_EMAIL, email, VERIFY_TTL).await?;
        let verify_link = link(state, "/verify-email", &token);
        send_later(
            state,
            email.to_string(),
            mail::verify_email_message(&verify_link),
        );
        Ok(())
    }
    .await;
    if let Err(error) = result {
        tracing::error!(error = %error, "failed to prepare verification email");
    }
}

// ── Registration ──

#[derive(Debug, Deserialize)]
struct RegisterBody {
    username: String,
    email: String,
    password: String,
}

async fn register(
    State(state): State<AppState>,
    client: ClientMeta,
    Json(body): Json<RegisterBody>,
) -> ApiResult {
    if !state.registration_open {
        return Err(ApiError::forbidden("Registration is closed"));
    }
    let username = accounts::validate_username(&body.username).map_err(ApiError::bad_request)?;
    let email = accounts::validate_email(&body.email).map_err(ApiError::bad_request)?;
    auth::validate_password(&body.password).map_err(ApiError::bad_request)?;
    allow_email(&state, &client, &email)?;

    // Hash before looking anything up so an address that already has an account costs the same.
    let password_hash = auth::hash_password_blocking(body.password)
        .await
        .map_err(|_| ApiError::internal("Failed to create account"))?;

    let taken = address_has_verified_owner(&state, &email)
        .await
        .map_err(db_error("Failed to create account"))?;
    if taken {
        let reset_url = format!("{}/forgot-password", state.public_base_url);
        send_later(&state, email, mail::address_in_use_message(&reset_url));
        return message(REGISTERED);
    }

    // New accounts start in the public tier, the same access as a visitor. An admin grants more.
    let user_id: Option<i32> = sqlx::query_scalar(
        "INSERT INTO users (username, password_hash, tier_id, email)
         SELECT $1, $2, id, $3 FROM tiers WHERE rank = 0
           AND NOT EXISTS (SELECT 1 FROM users WHERE lower(username) = lower($1))
         RETURNING id",
    )
    .bind(&username)
    .bind(password_hash)
    .bind(&email)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| {
        if is_unique_violation(&error) {
            ApiError::conflict("That username is already taken")
        } else {
            db_error("Failed to create account")(error)
        }
    })?;
    let user_id = user_id.ok_or_else(|| ApiError::conflict("That username is already taken"))?;

    audit::record(
        &state.pool,
        &client,
        Some(user_id),
        "user.registered",
        "user",
        user_id,
        json!({"username": username}),
    )
    .await;

    let background = state.clone();
    tokio::spawn(async move { mail_verification(&background, user_id, &email).await });
    message(REGISTERED)
}

const REGISTERED: &str = "Check your email for a link to confirm your address";

// ── Email verification ──

#[derive(Debug, Deserialize)]
struct TokenBody {
    token: String,
}

async fn verify_email(
    State(state): State<AppState>,
    client: ClientMeta,
    Json(body): Json<TokenBody>,
) -> ApiResult {
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(db_error("Failed to verify email"))?;
    let (user_id, email) = accounts::consume_token(&mut *tx, &body.token, VERIFY_EMAIL)
        .await
        .map_err(db_error("Failed to verify email"))?
        .ok_or_else(|| ApiError::bad_request(INVALID_LINK))?;

    sqlx::query("UPDATE users SET email = $2, email_verified_at = NOW() WHERE id = $1")
        .bind(user_id)
        .bind(&email)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            if is_unique_violation(&error) {
                ApiError::conflict("That email address is already in use")
            } else {
                db_error("Failed to verify email")(error)
            }
        })?;
    tx.commit()
        .await
        .map_err(db_error("Failed to verify email"))?;

    audit::record(
        &state.pool,
        &client,
        Some(user_id),
        "email.verified",
        "user",
        user_id,
        json!({}),
    )
    .await;
    message("Email confirmed. You can sign in now.")
}

#[derive(Debug, Deserialize)]
struct EmailBody {
    email: String,
}

const IF_REGISTERED: &str = "If that address needs confirming, a new link is on its way";

async fn resend_verification(
    State(state): State<AppState>,
    client: ClientMeta,
    Json(body): Json<EmailBody>,
) -> ApiResult {
    let email = accounts::validate_email(&body.email).map_err(ApiError::bad_request)?;
    allow_email(&state, &client, &email)?;

    tokio::spawn(async move {
        let pending: Result<Option<i32>, _> = sqlx::query_scalar(
            "SELECT id FROM users
             WHERE email_verified_at IS NULL AND lower(email) = lower($1)
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(&email)
        .fetch_optional(&state.pool)
        .await;
        match pending {
            Ok(Some(user_id)) => mail_verification(&state, user_id, &email).await,
            Ok(None) => {}
            Err(error) => tracing::error!(error = %error, "failed looking up pending account"),
        }
    });
    message(IF_REGISTERED)
}

// ── Changing the email address ──

#[derive(Debug, Deserialize)]
struct ChangeEmailBody {
    email: String,
    current_password: String,
}

/// Starts a change of address for the signed-in user. The new address only replaces the old one
/// once its link is opened, so a typo cannot lock anyone out of password recovery.
async fn change_email(
    State(state): State<AppState>,
    client: ClientMeta,
    actor: Actor,
    Json(body): Json<ChangeEmailBody>,
) -> ApiResult {
    let user_id = require_user(&actor)?;
    let email = accounts::validate_email(&body.email).map_err(ApiError::bad_request)?;

    let (username, current_hash) = sqlx::query_as::<_, (String, String)>(
        "SELECT username, password_hash FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(db_error("Failed to look up user"))?
    .ok_or_else(ApiError::unauthorized)?;

    let throttle = &state.login_throttle;
    if !throttle.allows(&username, &client.ip) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many attempts; try again later",
        ));
    }
    if !auth::verify_password_blocking(body.current_password, Some(current_hash)).await {
        throttle.record_failure(&username, &client.ip);
        audit::record(
            &state.pool,
            &client,
            Some(user_id),
            "email_change.failed",
            "user",
            user_id,
            json!({"reason": "invalid_password"}),
        )
        .await;
        return Err(ApiError::forbidden("Current password is incorrect"));
    }
    throttle.record_success(&username, &client.ip);
    allow_email(&state, &client, &email)?;

    audit::record(
        &state.pool,
        &client,
        Some(user_id),
        "email.change_requested",
        "user",
        user_id,
        json!({}),
    )
    .await;
    tokio::spawn(async move { mail_verification(&state, user_id, &email).await });
    message("Check the new address for a link to confirm the change")
}

// ── Password reset ──

async fn request_password_reset(
    State(state): State<AppState>,
    client: ClientMeta,
    Json(body): Json<EmailBody>,
) -> ApiResult {
    let email = accounts::validate_email(&body.email).map_err(ApiError::bad_request)?;
    allow_email(&state, &client, &email)?;

    tokio::spawn(async move {
        let user_id: Result<Option<i32>, _> = sqlx::query_scalar(
            "SELECT id FROM users WHERE email_verified_at IS NOT NULL AND lower(email) = lower($1)",
        )
        .bind(&email)
        .fetch_optional(&state.pool)
        .await;
        let user_id = match user_id {
            Ok(Some(user_id)) => user_id,
            Ok(None) => return,
            Err(error) => {
                tracing::error!(error = %error, "failed looking up account for reset");
                return;
            }
        };
        match accounts::issue_token(&state.pool, user_id, RESET_PASSWORD, &email, RESET_TTL).await {
            Ok(token) => {
                let reset_link = link(&state, "/reset-password", &token);
                send_later(&state, email, mail::reset_password_message(&reset_link));
                audit::record(
                    &state.pool,
                    &client,
                    Some(user_id),
                    "password_reset.requested",
                    "user",
                    user_id,
                    json!({}),
                )
                .await;
            }
            Err(error) => tracing::error!(error = %error, "failed creating reset token"),
        }
    });
    message("If that address has an account, a reset link is on its way")
}

#[derive(Debug, Deserialize)]
struct ResetBody {
    token: String,
    new_password: String,
}

async fn confirm_password_reset(
    State(state): State<AppState>,
    client: ClientMeta,
    Json(body): Json<ResetBody>,
) -> ApiResult {
    auth::validate_password(&body.new_password).map_err(ApiError::bad_request)?;
    let new_hash = auth::hash_password_blocking(body.new_password)
        .await
        .map_err(|_| ApiError::internal("Failed to reset password"))?;

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(db_error("Failed to reset password"))?;
    let (user_id, _) = accounts::consume_token(&mut *tx, &body.token, RESET_PASSWORD)
        .await
        .map_err(db_error("Failed to reset password"))?
        .ok_or_else(|| ApiError::bad_request(INVALID_LINK))?;

    sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
        .bind(new_hash)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(db_error("Failed to reset password"))?;
    // Whoever knew the old password, or held a session, loses access.
    auth::revoke_user_sessions(&mut *tx, user_id, None)
        .await
        .map_err(db_error("Failed to reset password"))?;
    tx.commit()
        .await
        .map_err(db_error("Failed to reset password"))?;

    audit::record(
        &state.pool,
        &client,
        Some(user_id),
        "password_reset.completed",
        "user",
        user_id,
        json!({}),
    )
    .await;
    message("Password changed. You can sign in with it now.")
}
