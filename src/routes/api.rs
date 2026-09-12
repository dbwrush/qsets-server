use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::{
    models::{QuestionPool, Tier, User},
    services::{audit, auth, generator, security},
    state::AppState,
};

const MAX_POOL_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/me", get(me))
        .route("/api/pools", get(list_pools).post(upload_pool))
        .route("/api/pools/:id", axum::routing::delete(delete_pool))
        .route("/api/pools/:id/books", get(pool_books))
        .route("/api/generate", post(generate))
        .route("/api/admin/tiers", get(list_tiers).post(create_tier))
        .route("/api/admin/tiers/:id", put(update_tier).delete(delete_tier))
        .route("/api/admin/users", get(list_users).post(create_user))
        .route("/api/admin/users/:id", put(update_user).delete(delete_user))
        .layer(DefaultBodyLimit::max(MAX_POOL_UPLOAD_BYTES))
        .with_state(state)
}

async fn require_admin(state: &AppState, jar: &CookieJar) -> Result<i32, Response> {
    let Some(user_id) = auth::get_user_id_from_jar(&state.pool, jar).await else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Not authenticated"})),
        )
            .into_response());
    };

    let is_admin = sqlx::query_scalar::<_, bool>("SELECT is_admin FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&state.pool)
        .await
        .unwrap_or(false);

    if !is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Admin required"})),
        )
            .into_response());
    }

    Ok(user_id)
}

async fn health() -> impl IntoResponse {
    Json(json!({"ok": true}))
}

async fn parsed_pool(
    state: &AppState,
    pool_id: i32,
) -> Option<Arc<Vec<generator::QuestionRecord>>> {
    let mut cache = state.parsed_pools.write().await;
    if let Some(cached) = cache.get(pool_id) {
        return Some(cached);
    }

    let csv_text =
        sqlx::query_scalar::<_, String>("SELECT csv_text FROM question_pools WHERE id = $1")
            .bind(pool_id)
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten()?;
    let parsed = Arc::new(generator::parse_csv(&csv_text));
    cache.insert(pool_id, parsed.clone());
    Some(parsed)
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(payload): Json<LoginRequest>,
) -> impl IntoResponse {
    if !security::verify_csrf(&jar, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "CSRF validation failed"})),
        )
            .into_response();
    }

    let key = format!("{}:{}", payload.username, security::client_ip(&headers));
    if !security::allow_login_attempt(&key).await {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error": "Too many login attempts"})),
        )
            .into_response();
    }

    let row = sqlx::query_as::<_, User>(
        "SELECT id, username, password_hash, is_admin, tier_id, created_at FROM users WHERE username = $1",
    )
    .bind(&payload.username)
    .fetch_optional(&state.pool)
    .await;

    let Ok(Some(user)) = row else {
        security::record_login_attempt(&key).await;
        audit::log_event(
            &state.pool,
            audit::AuditEvent {
                actor_user_id: None,
                action: "login.failed",
                target_type: "user",
                target_id: Some(payload.username),
                metadata: json!({"reason": "user_not_found"}),
                ip_address: Some(security::client_ip(&headers)),
                user_agent: security::user_agent(&headers),
            },
        )
        .await;
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Invalid credentials"})),
        )
            .into_response();
    };

    if !auth::verify_password(&payload.password, &user.password_hash) {
        security::record_login_attempt(&key).await;
        audit::log_event(
            &state.pool,
            audit::AuditEvent {
                actor_user_id: Some(user.id),
                action: "login.failed",
                target_type: "user",
                target_id: Some(user.id.to_string()),
                metadata: json!({"reason": "invalid_password"}),
                ip_address: Some(security::client_ip(&headers)),
                user_agent: security::user_agent(&headers),
            },
        )
        .await;
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Invalid credentials"})),
        )
            .into_response();
    }

    security::clear_login_attempts(&key).await;
    let token = match auth::create_session(&state.pool, user.id).await {
        Ok(t) => t,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to create session"})),
            )
                .into_response()
        }
    };

    audit::log_event(
        &state.pool,
        audit::AuditEvent {
            actor_user_id: Some(user.id),
            action: "login.success",
            target_type: "user",
            target_id: Some(user.id.to_string()),
            metadata: json!({}),
            ip_address: Some(security::client_ip(&headers)),
            user_agent: security::user_agent(&headers),
        },
    )
    .await;

    let jar = auth::add_session_cookie(jar, token);
    (jar, Json(json!({"message": "Logged in"}))).into_response()
}

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> impl IntoResponse {
    if !security::verify_csrf(&jar, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "CSRF validation failed"})),
        )
            .into_response();
    }

    let user_id = auth::get_user_id_from_jar(&state.pool, &jar).await;

    if let Some(token) = jar.get("qsets_session").map(|c| c.value().to_string()) {
        auth::destroy_session(&state.pool, &token).await;
    }

    if let Some(actor) = user_id {
        audit::log_event(
            &state.pool,
            audit::AuditEvent {
                actor_user_id: Some(actor),
                action: "logout",
                target_type: "user",
                target_id: Some(actor.to_string()),
                metadata: json!({}),
                ip_address: Some(security::client_ip(&headers)),
                user_agent: security::user_agent(&headers),
            },
        )
        .await;
    }

    let jar = auth::clear_session_cookie(jar);
    (jar, Json(json!({"message": "Logged out"}))).into_response()
}

async fn me(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    let Some(user_id) = auth::get_user_id_from_jar(&state.pool, &jar).await else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Not authenticated"})),
        )
            .into_response();
    };

    let user = sqlx::query_as::<_, User>(
        "SELECT id, username, password_hash, is_admin, tier_id, created_at FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await;

    match user {
        Ok(u) => (
            StatusCode::OK,
            Json(json!({
                "id": u.id,
                "username": u.username,
                "is_admin": u.is_admin,
                "tier_id": u.tier_id,
            })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Lookup failed"})),
        )
            .into_response(),
    }
}

async fn list_pools(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    let user_tier_rank = if let Some(user_id) = auth::get_user_id_from_jar(&state.pool, &jar).await
    {
        sqlx::query_scalar::<_, i32>(
            "SELECT t.rank FROM users u JOIN tiers t ON u.tier_id = t.id WHERE u.id = $1",
        )
        .bind(user_id)
        .fetch_one(&state.pool)
        .await
        .unwrap_or(0)
    } else {
        0
    };

    let pools = sqlx::query_as::<_, (i32, String, i32, String)>(
        "SELECT qp.id, qp.name, qp.tier_id, t.name as tier_name
         FROM question_pools qp
         JOIN tiers t ON qp.tier_id = t.id
         WHERE t.rank <= $1
         ORDER BY qp.created_at DESC",
    )
    .bind(user_tier_rank)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let lightweight: Vec<_> = pools
        .into_iter()
        .map(|(id, name, tier_id, tier_name)| {
            json!({"id": id, "name": name, "tier_id": tier_id, "tier_name": tier_name})
        })
        .collect();

    (StatusCode::OK, Json(json!({"pools": lightweight}))).into_response()
}

async fn upload_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    mut multipart: Multipart,
) -> impl IntoResponse {
    if !security::verify_csrf(&jar, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "CSRF validation failed"})),
        )
            .into_response();
    }

    let user_id = match require_admin(&state, &jar).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let mut pool_name: Option<String> = None;
    let mut tier_id: Option<i32> = None;
    let mut csv_data: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or_default() {
            "name" => {
                pool_name = field.text().await.ok();
            }
            "tier_id" => {
                tier_id = field.text().await.ok().and_then(|v| v.parse::<i32>().ok());
            }
            "file" => {
                csv_data = field.text().await.ok();
            }
            _ => {}
        }
    }

    let (Some(name), Some(tier), Some(csv)) = (pool_name, tier_id, csv_data) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Missing fields"})),
        )
            .into_response();
    };

    // Validate the CSV before storing
    let validation = generator::validate_csv(&csv);
    if !validation.errors.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": validation.errors.join("; "),
                "validation": validation,
            })),
        )
            .into_response();
    }

    let inserted = sqlx::query_as::<_, QuestionPool>(
        "INSERT INTO question_pools (name, tier_id, csv_text)
            VALUES ($1, $2, $3)
            RETURNING id, name, tier_id, csv_text, created_at, updated_at",
    )
    .bind(&name)
    .bind(tier)
    .bind(&csv)
    .fetch_one(&state.pool)
    .await;

    match inserted {
        Ok(pool) => {
            audit::log_event(
                &state.pool,
                audit::AuditEvent {
                    actor_user_id: Some(user_id),
                    action: "pool.uploaded",
                    target_type: "question_pool",
                    target_id: Some(pool.id.to_string()),
                    metadata: json!({"name": pool.name}),
                    ip_address: Some(security::client_ip(&headers)),
                    user_agent: security::user_agent(&headers),
                },
            )
            .await;

            (
                StatusCode::OK,
                Json(json!({
                    "id": pool.id,
                    "name": pool.name,
                    "tier_id": pool.tier_id,
                    "validation": {
                        "valid_count": validation.valid_count,
                        "skipped_count": validation.skipped_count,
                        "warnings": validation.warnings
                    }
                })),
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Insert failed"})),
        )
            .into_response(),
    }
}

async fn delete_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(id): Path<i32>,
) -> impl IntoResponse {
    if !security::verify_csrf(&jar, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "CSRF validation failed"})),
        )
            .into_response();
    }

    let actor = match require_admin(&state, &jar).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let deleted =
        sqlx::query_scalar::<_, i32>("DELETE FROM question_pools WHERE id = $1 RETURNING id")
            .bind(id)
            .fetch_optional(&state.pool)
            .await;

    match deleted {
        Ok(Some(pool_id)) => {
            state.parsed_pools.write().await.remove(pool_id);
            audit::log_event(
                &state.pool,
                audit::AuditEvent {
                    actor_user_id: Some(actor),
                    action: "pool.deleted",
                    target_type: "question_pool",
                    target_id: Some(pool_id.to_string()),
                    metadata: json!({}),
                    ip_address: Some(security::client_ip(&headers)),
                    user_agent: security::user_agent(&headers),
                },
            )
            .await;
            (StatusCode::OK, Json(json!({"id": pool_id}))).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Pool not found"})),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(pool_id = id, error = %error, "failed to delete pool");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to delete pool"})),
            )
                .into_response()
        }
    }
}

async fn pool_books(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    jar: CookieJar,
) -> impl IntoResponse {
    let user_id = auth::get_user_id_from_jar(&state.pool, &jar).await;

    let rows = sqlx::query_as::<_, (i32, String, i32)>(
        "SELECT id, name, tier_id FROM question_pools WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await;

    let Ok(Some((pool_id, _pool_name, pool_tier_id))) = rows else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Pool not found"})),
        )
            .into_response();
    };

    let tiers = sqlx::query_as::<_, Tier>("SELECT id, name, rank FROM tiers")
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

    let user_rank = if let Some(uid) = user_id {
        sqlx::query_scalar::<_, i32>(
            "SELECT t.rank FROM users u JOIN tiers t ON u.tier_id = t.id WHERE u.id = $1",
        )
        .bind(uid)
        .fetch_one(&state.pool)
        .await
        .unwrap_or(0)
    } else {
        0
    };

    let pool_rank = tiers
        .iter()
        .find(|t| t.id == pool_tier_id)
        .map(|t| t.rank)
        .unwrap_or(0);

    if pool_rank > user_rank {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Tier access denied"})),
        )
            .into_response();
    }

    let Some(parsed) = parsed_pool(&state, pool_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Pool not found"})),
        )
            .into_response();
    };
    let books = generator::list_books(parsed.as_ref());
    (StatusCode::OK, Json(json!({"books": books}))).into_response()
}

#[derive(Debug, Deserialize)]
struct GenerateBody {
    pool_id: i32,
    question_type: String,
    count: usize,
    situation: Option<bool>,
    seed: Option<u64>,
    books: Option<Vec<generator::BookFilter>>,
}

async fn generate(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<GenerateBody>,
) -> impl IntoResponse {
    if !security::verify_csrf(&jar, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "CSRF validation failed"})),
        )
            .into_response();
    }

    let user_id = auth::get_user_id_from_jar(&state.pool, &jar).await;

    let rows = sqlx::query_as::<_, (i32, String, i32)>(
        "SELECT id, name, tier_id FROM question_pools WHERE id = $1",
    )
    .bind(body.pool_id)
    .fetch_optional(&state.pool)
    .await;

    let Ok(Some((pool_id, _pool_name, pool_tier_id))) = rows else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Pool not found"})),
        )
            .into_response();
    };

    let tiers = sqlx::query_as::<_, Tier>("SELECT id, name, rank FROM tiers")
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

    let user_rank = if let Some(uid) = user_id {
        sqlx::query_scalar::<_, i32>(
            "SELECT t.rank FROM users u JOIN tiers t ON u.tier_id = t.id WHERE u.id = $1",
        )
        .bind(uid)
        .fetch_one(&state.pool)
        .await
        .unwrap_or(0)
    } else {
        0
    };

    let pool_rank = tiers
        .iter()
        .find(|t| t.id == pool_tier_id)
        .map(|t| t.rank)
        .unwrap_or(0);

    if pool_rank > user_rank {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Tier access denied"})),
        )
            .into_response();
    }

    let Some(parsed) = parsed_pool(&state, pool_id).await else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Pool not found"})),
        )
            .into_response();
    };
    enum GenerationSource {
        Cached(Arc<Vec<generator::QuestionRecord>>),
        Filtered(Vec<generator::QuestionRecord>),
    }

    let generation_source = match body.books.as_ref() {
        Some(filters) if !filters.is_empty() => {
            match generator::filter_by_books_cow(parsed.as_ref(), filters) {
                std::borrow::Cow::Borrowed(_) => GenerationSource::Cached(parsed.clone()),
                std::borrow::Cow::Owned(filtered) if filtered.is_empty() => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error": "No questions match selected books/chapters"})),
                    )
                        .into_response();
                }
                std::borrow::Cow::Owned(filtered) => GenerationSource::Filtered(filtered),
            }
        }
        _ if parsed.is_empty() => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "No questions match selected books/chapters"})),
            )
                .into_response();
        }
        _ => GenerationSource::Cached(parsed.clone()),
    };

    let req = generator::GenerateRequest {
        question_type: body.question_type,
        count: body.count,
        situation: body.situation,
        seed: body.seed,
    };
    let generation_permit = match state.generation_slots.clone().acquire_owned().await {
        Ok(permit) => permit,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "Question generation is unavailable"})),
            )
                .into_response()
        }
    };
    let generated = match tokio::task::spawn_blocking(move || {
        let _generation_permit = generation_permit;
        match generation_source {
            GenerationSource::Cached(pool) => generator::generate_questions(pool.as_ref(), &req),
            GenerationSource::Filtered(pool) => generator::generate_questions(&pool, &req),
        }
    })
    .await
    {
        Ok(generated) => generated,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Question generation failed"})),
            )
                .into_response()
        }
    };

    audit::log_event(
        &state.pool,
        audit::AuditEvent {
            actor_user_id: user_id,
            action: "questions.generated",
            target_type: "question_pool",
            target_id: Some(pool_id.to_string()),
            metadata: json!({"count": generated.len()}),
            ip_address: Some(security::client_ip(&headers)),
            user_agent: security::user_agent(&headers),
        },
    )
    .await;

    (StatusCode::OK, Json(json!({"questions": generated}))).into_response()
}

#[derive(Debug, Deserialize)]
struct TierCreateBody {
    name: String,
    rank: i32,
}

#[derive(Debug, Deserialize)]
struct TierUpdateBody {
    name: String,
    rank: i32,
}

async fn list_tiers(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    if let Err(resp) = require_admin(&state, &jar).await {
        return resp;
    }

    let tiers = sqlx::query_as::<_, Tier>("SELECT id, name, rank FROM tiers ORDER BY rank ASC")
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

    (StatusCode::OK, Json(json!({"tiers": tiers}))).into_response()
}

async fn create_tier(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<TierCreateBody>,
) -> impl IntoResponse {
    if !security::verify_csrf(&jar, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "CSRF validation failed"})),
        )
            .into_response();
    }

    let actor = match require_admin(&state, &jar).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let created = sqlx::query_as::<_, Tier>(
        "INSERT INTO tiers (name, rank) VALUES ($1, $2) RETURNING id, name, rank",
    )
    .bind(body.name)
    .bind(body.rank)
    .fetch_one(&state.pool)
    .await;

    match created {
        Ok(tier) => {
            audit::log_event(
                &state.pool,
                audit::AuditEvent {
                    actor_user_id: Some(actor),
                    action: "tier.created",
                    target_type: "tier",
                    target_id: Some(tier.id.to_string()),
                    metadata: json!({"name": tier.name, "rank": tier.rank}),
                    ip_address: Some(security::client_ip(&headers)),
                    user_agent: security::user_agent(&headers),
                },
            )
            .await;
            (StatusCode::OK, Json(json!({"tier": tier}))).into_response()
        }
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Failed to create tier"})),
        )
            .into_response(),
    }
}

async fn update_tier(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<TierUpdateBody>,
) -> impl IntoResponse {
    if !security::verify_csrf(&jar, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "CSRF validation failed"})),
        )
            .into_response();
    }

    let actor = match require_admin(&state, &jar).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let updated = sqlx::query_as::<_, Tier>(
        "UPDATE tiers SET name = $1, rank = $2 WHERE id = $3 RETURNING id, name, rank",
    )
    .bind(body.name)
    .bind(body.rank)
    .bind(id)
    .fetch_optional(&state.pool)
    .await;

    match updated {
        Ok(Some(tier)) => {
            audit::log_event(
                &state.pool,
                audit::AuditEvent {
                    actor_user_id: Some(actor),
                    action: "tier.updated",
                    target_type: "tier",
                    target_id: Some(tier.id.to_string()),
                    metadata: json!({"name": tier.name, "rank": tier.rank}),
                    ip_address: Some(security::client_ip(&headers)),
                    user_agent: security::user_agent(&headers),
                },
            )
            .await;
            (StatusCode::OK, Json(json!({"tier": tier}))).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Tier not found"})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Failed to update tier"})),
        )
            .into_response(),
    }
}

async fn delete_tier(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    headers: HeaderMap,
    jar: CookieJar,
) -> impl IntoResponse {
    if !security::verify_csrf(&jar, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "CSRF validation failed"})),
        )
            .into_response();
    }

    let actor = match require_admin(&state, &jar).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let deleted = sqlx::query_scalar::<_, i32>("DELETE FROM tiers WHERE id = $1 RETURNING id")
        .bind(id)
        .fetch_optional(&state.pool)
        .await;

    match deleted {
        Ok(Some(tier_id)) => {
            audit::log_event(
                &state.pool,
                audit::AuditEvent {
                    actor_user_id: Some(actor),
                    action: "tier.deleted",
                    target_type: "tier",
                    target_id: Some(tier_id.to_string()),
                    metadata: json!({}),
                    ip_address: Some(security::client_ip(&headers)),
                    user_agent: security::user_agent(&headers),
                },
            )
            .await;
            (StatusCode::OK, Json(json!({"deleted": tier_id}))).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Tier not found"})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Failed to delete tier (possibly in use)"})),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct CreateUserBody {
    username: String,
    password: String,
    tier_id: i32,
    is_admin: bool,
}

#[derive(Debug, Deserialize)]
struct UpdateUserBody {
    username: String,
    tier_id: i32,
    is_admin: bool,
    password: Option<String>,
}

async fn list_users(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    if let Err(resp) = require_admin(&state, &jar).await {
        return resp;
    }

    let users = sqlx::query_as::<_, User>(
        "SELECT id, username, password_hash, is_admin, tier_id, created_at FROM users ORDER BY created_at DESC",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let safe: Vec<_> = users
        .into_iter()
        .map(|u| json!({"id": u.id, "username": u.username, "tier_id": u.tier_id, "is_admin": u.is_admin, "created_at": u.created_at}))
        .collect();

    (StatusCode::OK, Json(json!({"users": safe}))).into_response()
}

async fn create_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<CreateUserBody>,
) -> impl IntoResponse {
    if !security::verify_csrf(&jar, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "CSRF validation failed"})),
        )
            .into_response();
    }

    let actor = match require_admin(&state, &jar).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let Ok(password_hash) = auth::hash_password(&body.password) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid password"})),
        )
            .into_response();
    };

    let inserted = sqlx::query_as::<_, User>(
        "INSERT INTO users (username, password_hash, tier_id, is_admin)
         VALUES ($1, $2, $3, $4)
         RETURNING id, username, password_hash, is_admin, tier_id, created_at",
    )
    .bind(body.username)
    .bind(password_hash)
    .bind(body.tier_id)
    .bind(body.is_admin)
    .fetch_one(&state.pool)
    .await;

    match inserted {
        Ok(user) => {
            audit::log_event(
                &state.pool,
                audit::AuditEvent {
                    actor_user_id: Some(actor),
                    action: "user.created",
                    target_type: "user",
                    target_id: Some(user.id.to_string()),
                    metadata: json!({"username": user.username, "tier_id": user.tier_id, "is_admin": user.is_admin}),
                    ip_address: Some(security::client_ip(&headers)),
                    user_agent: security::user_agent(&headers),
                },
            )
            .await;

            (
                StatusCode::OK,
                Json(json!({
                    "id": user.id,
                    "username": user.username,
                    "tier_id": user.tier_id,
                    "is_admin": user.is_admin
                })),
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Failed to create user"})),
        )
            .into_response(),
    }
}

async fn update_user(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<UpdateUserBody>,
) -> impl IntoResponse {
    if !security::verify_csrf(&jar, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "CSRF validation failed"})),
        )
            .into_response();
    }

    let actor = match require_admin(&state, &jar).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    if actor == id && !body.is_admin {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "You cannot remove your own admin role"})),
        )
            .into_response();
    }

    let updated = if let Some(password) = body.password.as_ref().filter(|p| !p.trim().is_empty()) {
        let Ok(password_hash) = auth::hash_password(password) else {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "Invalid password"})),
            )
                .into_response();
        };

        sqlx::query_as::<_, User>(
            "UPDATE users
             SET username = $1, tier_id = $2, is_admin = $3, password_hash = $4
             WHERE id = $5
             RETURNING id, username, password_hash, is_admin, tier_id, created_at",
        )
        .bind(body.username)
        .bind(body.tier_id)
        .bind(body.is_admin)
        .bind(password_hash)
        .bind(id)
        .fetch_optional(&state.pool)
        .await
    } else {
        sqlx::query_as::<_, User>(
            "UPDATE users
             SET username = $1, tier_id = $2, is_admin = $3
             WHERE id = $4
             RETURNING id, username, password_hash, is_admin, tier_id, created_at",
        )
        .bind(body.username)
        .bind(body.tier_id)
        .bind(body.is_admin)
        .bind(id)
        .fetch_optional(&state.pool)
        .await
    };

    match updated {
        Ok(Some(user)) => {
            audit::log_event(
                &state.pool,
                audit::AuditEvent {
                    actor_user_id: Some(actor),
                    action: "user.updated",
                    target_type: "user",
                    target_id: Some(user.id.to_string()),
                    metadata: json!({"username": user.username, "tier_id": user.tier_id, "is_admin": user.is_admin}),
                    ip_address: Some(security::client_ip(&headers)),
                    user_agent: security::user_agent(&headers),
                },
            )
            .await;

            (
                StatusCode::OK,
                Json(json!({
                    "id": user.id,
                    "username": user.username,
                    "tier_id": user.tier_id,
                    "is_admin": user.is_admin
                })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "User not found"})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Failed to update user"})),
        )
            .into_response(),
    }
}

async fn delete_user(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    headers: HeaderMap,
    jar: CookieJar,
) -> impl IntoResponse {
    if !security::verify_csrf(&jar, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "CSRF validation failed"})),
        )
            .into_response();
    }

    let actor = match require_admin(&state, &jar).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    if actor == id {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "You cannot delete your own account"})),
        )
            .into_response();
    }

    let deleted = sqlx::query_scalar::<_, i32>("DELETE FROM users WHERE id = $1 RETURNING id")
        .bind(id)
        .fetch_optional(&state.pool)
        .await;

    match deleted {
        Ok(Some(user_id)) => {
            audit::log_event(
                &state.pool,
                audit::AuditEvent {
                    actor_user_id: Some(actor),
                    action: "user.deleted",
                    target_type: "user",
                    target_id: Some(user_id.to_string()),
                    metadata: json!({}),
                    ip_address: Some(security::client_ip(&headers)),
                    user_agent: security::user_agent(&headers),
                },
            )
            .await;

            (StatusCode::OK, Json(json!({"deleted": user_id}))).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "User not found"})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Failed to delete user"})),
        )
            .into_response(),
    }
}
