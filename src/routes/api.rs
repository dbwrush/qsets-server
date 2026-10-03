use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use super::common::{
    db_error, is_foreign_key_violation, is_unique_violation, require_csrf, require_user, Admin,
    ApiError, ApiResult,
};
use crate::{
    models::{QuestionPool, Tier, User},
    services::{
        audit, auth,
        authz::{self, Actor, PoolAccess},
        generator,
        security::ClientMeta,
    },
    state::AppState,
};

const MAX_POOL_UPLOAD_BYTES: usize = 10 * 1024 * 1024;
/// Most sets one generate request may ask for; matches the limit in the generator form.
const MAX_SETS_PER_REQUEST: usize = 20;
const DEFAULT_AUDIT_PAGE: i64 = 50;
const MAX_AUDIT_PAGE: i64 = 200;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/me", get(me))
        .route("/api/me/password", post(change_own_password))
        .route("/api/pools", get(list_pools).post(upload_pool))
        .route("/api/pools/:id", put(replace_pool).delete(delete_pool))
        .route("/api/pools/:id/books", get(pool_books))
        .route("/api/tiers", get(list_assignable_tiers))
        .route("/api/generate", post(generate))
        .route("/api/admin/tiers", get(list_tiers).post(create_tier))
        .route("/api/admin/tiers/:id", put(update_tier).delete(delete_tier))
        .route("/api/admin/users", get(list_users).post(create_user))
        .route("/api/admin/users/:id", put(update_user).delete(delete_user))
        .route("/api/admin/audit", get(list_audit))
        .route_layer(middleware::from_fn(require_csrf))
        .layer(DefaultBodyLimit::max(MAX_POOL_UPLOAD_BYTES))
        .with_state(state)
}

fn ok(body: Value) -> ApiResult {
    Ok(Json(body).into_response())
}

fn pool_not_found() -> ApiError {
    ApiError::not_found("Pool not found")
}

fn tier_not_found() -> ApiError {
    ApiError::not_found("Tier not found")
}

fn user_not_found() -> ApiError {
    ApiError::not_found("User not found")
}

async fn health() -> impl IntoResponse {
    Json(json!({"ok": true}))
}

async fn load_pool(state: &AppState, pool_id: i32) -> ApiResult<PoolAccess> {
    authz::load_pool_access(&state.pool, pool_id)
        .await
        .map_err(db_error("Failed to load pool"))?
        .ok_or_else(pool_not_found)
}

/// Single gate for reading a pool: rejects pools above the caller's tier.
async fn authorize_pool_access(
    state: &AppState,
    actor: &Actor,
    pool_id: i32,
) -> ApiResult<PoolAccess> {
    let pool = load_pool(state, pool_id).await?;
    if !authz::can_access_pool(actor, pool.tier_rank) {
        return Err(ApiError::forbidden("Tier access denied"));
    }
    Ok(pool)
}

/// The pool's parsed questions, from the cache when it holds this version of the pool.
async fn parsed_pool(
    state: &AppState,
    pool: &PoolAccess,
) -> ApiResult<Arc<Vec<generator::QuestionRecord>>> {
    if let Some(cached) = state
        .parsed_pools
        .read()
        .await
        .get(pool.id, pool.updated_at)
    {
        return Ok(cached);
    }

    // Load and parse without holding the lock so cache hits for other pools are never blocked.
    // Concurrent misses for the same pool may both parse it; the second insert is harmless.
    let (csv_text, version) = sqlx::query_as::<_, (String, chrono::DateTime<chrono::Utc>)>(
        "SELECT csv_text, updated_at FROM question_pools WHERE id = $1",
    )
    .bind(pool.id)
    .fetch_optional(&state.pool)
    .await
    .map_err(db_error("Failed to load pool"))?
    .ok_or_else(pool_not_found)?;
    let parsed = Arc::new(generator::parse_csv(&csv_text));
    state
        .parsed_pools
        .write()
        .await
        .insert(pool.id, version, parsed.clone());
    Ok(parsed)
}

// ── Sessions ──

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
}

async fn login(
    State(state): State<AppState>,
    client: ClientMeta,
    jar: CookieJar,
    Json(payload): Json<LoginRequest>,
) -> ApiResult {
    let throttle = &state.login_throttle;
    if !throttle.allows(&payload.username, &client.ip) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many login attempts",
        ));
    }

    let user = sqlx::query_as::<_, User>(
        "SELECT id, username, password_hash, is_admin, can_upload_pools, tier_id, created_at FROM users WHERE username = $1",
    )
    .bind(&payload.username)
    .fetch_optional(&state.pool)
    .await
    .map_err(db_error("Failed to look up user"))?;

    // Unknown usernames still run a hash check so timing does not reveal which names exist.
    let hash = user.as_ref().map(|u| u.password_hash.clone());
    let verified = auth::verify_password_blocking(payload.password, hash).await;

    let user = match user {
        Some(user) if verified => user,
        user => {
            throttle.record_failure(&payload.username, &client.ip);
            let (actor, target, reason) = match &user {
                Some(u) => (Some(u.id), u.id.to_string(), "invalid_password"),
                None => (None, payload.username.clone(), "user_not_found"),
            };
            audit::record(
                &state.pool,
                &client,
                actor,
                "login.failed",
                "user",
                target,
                json!({"reason": reason}),
            )
            .await;
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "Invalid credentials",
            ));
        }
    };

    throttle.record_success(&payload.username, &client.ip);
    let token = auth::create_session(&state.pool, user.id)
        .await
        .map_err(|_| ApiError::internal("Failed to create session"))?;

    audit::record(
        &state.pool,
        &client,
        Some(user.id),
        "login.success",
        "user",
        user.id,
        json!({}),
    )
    .await;

    let jar = auth::add_session_cookie(jar, token);
    Ok((jar, Json(json!({"message": "Logged in"}))).into_response())
}

async fn logout(State(state): State<AppState>, client: ClientMeta, jar: CookieJar) -> Response {
    let user_id = auth::get_user_id_from_jar(&state.pool, &jar).await;

    if let Some(token) = jar.get(auth::SESSION_COOKIE).map(|c| c.value().to_string()) {
        auth::destroy_session(&state.pool, &token).await;
    }

    if let Some(actor) = user_id {
        audit::record(
            &state.pool,
            &client,
            Some(actor),
            "logout",
            "user",
            actor,
            json!({}),
        )
        .await;
    }

    let jar = auth::clear_session_cookie(jar);
    (jar, Json(json!({"message": "Logged out"}))).into_response()
}

async fn me(State(state): State<AppState>, actor: Actor) -> ApiResult {
    let user_id = require_user(&actor)?;
    let user = sqlx::query_as::<_, User>(
        "SELECT id, username, password_hash, is_admin, can_upload_pools, tier_id, created_at FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(db_error("Lookup failed"))?;

    ok(json!({
        "id": user.id,
        "username": user.username,
        "is_admin": user.is_admin,
        "can_upload_pools": user.can_upload_pools,
        "tier_id": user.tier_id,
    }))
}

#[derive(Debug, Deserialize)]
struct ChangePasswordBody {
    current_password: String,
    new_password: String,
}

/// Lets any signed-in user change their own password. The current password is required, and
/// wrong guesses count against the login throttle, so a stolen session cannot be used to guess
/// it or to lock the owner out of their account without it.
async fn change_own_password(
    State(state): State<AppState>,
    client: ClientMeta,
    actor: Actor,
    jar: CookieJar,
    Json(body): Json<ChangePasswordBody>,
) -> ApiResult {
    let user_id = require_user(&actor)?;
    auth::validate_password(&body.new_password).map_err(ApiError::bad_request)?;

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
            "password_change.failed",
            "user",
            user_id,
            json!({"reason": "invalid_password"}),
        )
        .await;
        return Err(ApiError::forbidden("Current password is incorrect"));
    }
    throttle.record_success(&username, &client.ip);

    let new_hash = auth::hash_password_blocking(body.new_password)
        .await
        .map_err(|_| ApiError::internal("Failed to change password"))?;
    let changed = auth::set_password(&state.pool, user_id, &new_hash, auth::session_token(&jar))
        .await
        .map_err(db_error("Failed to change password"))?;
    if !changed {
        return Err(user_not_found());
    }

    audit::record(
        &state.pool,
        &client,
        Some(user_id),
        "user.password_changed",
        "user",
        user_id,
        json!({"self_service": true}),
    )
    .await;

    ok(json!({"message": "Password changed"}))
}

// ── Pools ──

/// Tiers the caller is permitted to assign when uploading a pool.
async fn list_assignable_tiers(State(state): State<AppState>, actor: Actor) -> ApiResult {
    let tiers = sqlx::query_as::<_, Tier>(
        "SELECT id, name, rank FROM tiers WHERE $1 OR rank <= $2 ORDER BY rank ASC",
    )
    .bind(actor.is_admin)
    .bind(actor.tier_rank)
    .fetch_all(&state.pool)
    .await
    .map_err(db_error("Failed to load tiers"))?;

    ok(json!({
        "tiers": tiers,
        "can_upload_pools": authz::can_upload(&actor),
    }))
}

async fn list_pools(State(state): State<AppState>, actor: Actor) -> ApiResult {
    #[derive(sqlx::FromRow)]
    struct PoolRow {
        id: i32,
        name: String,
        tier_id: i32,
        tier_name: String,
        tier_rank: i32,
        created_by: Option<i32>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let pools = sqlx::query_as::<_, PoolRow>(
        "SELECT qp.id, qp.name, qp.tier_id, t.name AS tier_name, t.rank AS tier_rank,
                qp.created_by, qp.updated_at
         FROM question_pools qp
         JOIN tiers t ON qp.tier_id = t.id
         WHERE $1 OR t.rank <= $2
         ORDER BY qp.created_at DESC",
    )
    .bind(actor.is_admin)
    .bind(actor.tier_rank)
    .fetch_all(&state.pool)
    .await
    .map_err(db_error("Failed to load pools"))?;

    let lightweight: Vec<_> = pools
        .into_iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "tier_id": p.tier_id,
                "tier_name": p.tier_name,
                "tier_rank": p.tier_rank,
                "updated_at": p.updated_at,
                "can_manage": authz::can_manage_pool(&actor, p.created_by, p.tier_rank),
            })
        })
        .collect();

    ok(json!({"pools": lightweight}))
}

#[derive(Default)]
struct PoolForm {
    name: Option<String>,
    tier_id: Option<i32>,
    csv: Option<String>,
}

async fn read_pool_form(mut multipart: Multipart) -> ApiResult<PoolForm> {
    let mut form = PoolForm::default();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::new(e.status(), e.body_text()))?
    {
        let name = field.name().unwrap_or_default().to_string();
        let text = field
            .text()
            .await
            .map_err(|e| ApiError::new(e.status(), e.body_text()))?;
        match name.as_str() {
            "name" => form.name = Some(text),
            "tier_id" => form.tier_id = text.trim().parse().ok(),
            "file" => form.csv = Some(text),
            _ => {}
        }
    }
    Ok(form)
}

fn validate_pool_csv(csv: &str) -> ApiResult<generator::CsvValidation> {
    let validation = generator::validate_csv(csv);
    if !validation.errors.is_empty() {
        return Err(ApiError::bad_request(validation.errors.join("; "))
            .with_details(json!({"validation": validation})));
    }
    Ok(validation)
}

fn validation_summary(validation: &generator::CsvValidation) -> Value {
    json!({
        "valid_count": validation.valid_count,
        "skipped_count": validation.skipped_count,
        "warnings": validation.warnings,
    })
}

async fn upload_pool(
    State(state): State<AppState>,
    client: ClientMeta,
    actor: Actor,
    multipart: Multipart,
) -> ApiResult {
    if !authz::can_upload(&actor) {
        return Err(ApiError::forbidden(
            "You do not have permission to upload question pools",
        ));
    }
    let user_id = require_user(&actor)?;

    let form = read_pool_form(multipart).await?;
    let (Some(name), Some(tier), Some(csv)) = (form.name, form.tier_id, form.csv) else {
        return Err(ApiError::bad_request("Missing fields"));
    };
    let name = name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("Pool name must not be empty"));
    }

    // The requested tier is never trusted from the client; re-check it against the uploader.
    let requested_tier_rank = authz::tier_rank(&state.pool, tier)
        .await
        .map_err(db_error("Failed to validate tier"))?
        .ok_or_else(|| ApiError::bad_request("Unknown tier"))?;
    if !authz::can_assign_pool_tier(&actor, requested_tier_rank) {
        return Err(ApiError::forbidden(
            "You cannot create a pool above your own tier",
        ));
    }

    let validation = validate_pool_csv(&csv)?;

    let pool = sqlx::query_as::<_, QuestionPool>(
        "INSERT INTO question_pools (name, tier_id, csv_text, created_by)
            VALUES ($1, $2, $3, $4)
            RETURNING id, name, tier_id, csv_text, created_by, created_at, updated_at",
    )
    .bind(name)
    .bind(tier)
    .bind(&csv)
    .bind(user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| {
        if is_unique_violation(&error) {
            ApiError::conflict(
                "A pool with that name already exists; replace it or choose another name",
            )
        } else {
            db_error("Insert failed")(error)
        }
    })?;

    audit::record(
        &state.pool,
        &client,
        Some(user_id),
        "pool.uploaded",
        "question_pool",
        pool.id,
        json!({"name": pool.name, "tier_id": pool.tier_id}),
    )
    .await;

    ok(json!({
        "id": pool.id,
        "name": pool.name,
        "tier_id": pool.tier_id,
        "validation": validation_summary(&validation),
    }))
}

/// Swaps a pool's CSV in place, keeping its id, name and tier.
async fn replace_pool(
    State(state): State<AppState>,
    client: ClientMeta,
    actor: Actor,
    Path(id): Path<i32>,
    multipart: Multipart,
) -> ApiResult {
    let user_id = require_user(&actor)?;
    let pool = load_pool(&state, id).await?;
    if !authz::can_manage_pool(&actor, pool.created_by, pool.tier_rank) {
        return Err(ApiError::forbidden("You cannot replace this pool"));
    }

    let csv = read_pool_form(multipart)
        .await?
        .csv
        .ok_or_else(|| ApiError::bad_request("Missing file"))?;
    let validation = validate_pool_csv(&csv)?;

    sqlx::query_scalar::<_, i32>(
        "UPDATE question_pools SET csv_text = $1, updated_at = NOW() WHERE id = $2 RETURNING id",
    )
    .bind(&csv)
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(db_error("Failed to replace pool"))?
    .ok_or_else(pool_not_found)?;
    // The new version already misses the cache; dropping the old entry just frees it sooner.
    state.parsed_pools.write().await.remove(id);

    audit::record(
        &state.pool,
        &client,
        Some(user_id),
        "pool.replaced",
        "question_pool",
        id,
        json!({
            "name": pool.name,
            "valid_count": validation.valid_count,
            "skipped_count": validation.skipped_count,
        }),
    )
    .await;

    ok(json!({
        "id": pool.id,
        "name": pool.name,
        "tier_id": pool.tier_id,
        "validation": validation_summary(&validation),
    }))
}

async fn delete_pool(
    State(state): State<AppState>,
    client: ClientMeta,
    actor: Actor,
    Path(id): Path<i32>,
) -> ApiResult {
    let pool = load_pool(&state, id).await?;
    if !authz::can_manage_pool(&actor, pool.created_by, pool.tier_rank) {
        return Err(ApiError::forbidden("You cannot delete this pool"));
    }

    let pool_id =
        sqlx::query_scalar::<_, i32>("DELETE FROM question_pools WHERE id = $1 RETURNING id")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(db_error("Failed to delete pool"))?
            .ok_or_else(pool_not_found)?;
    state.parsed_pools.write().await.remove(pool_id);

    audit::record(
        &state.pool,
        &client,
        actor.user_id,
        "pool.deleted",
        "question_pool",
        pool_id,
        json!({"name": pool.name}),
    )
    .await;
    ok(json!({"id": pool_id}))
}

async fn pool_books(State(state): State<AppState>, actor: Actor, Path(id): Path<i32>) -> ApiResult {
    let pool = authorize_pool_access(&state, &actor, id).await?;
    let parsed = parsed_pool(&state, &pool).await?;
    ok(json!({
        "books": generator::list_books(parsed.as_ref()),
        "pool": {
            "id": pool.id,
            "name": pool.name,
            "tier_id": pool.tier_id,
            "tier_name": pool.tier_name,
        },
    }))
}

fn one_set() -> usize {
    1
}

#[derive(Debug, Deserialize)]
struct GenerateBody {
    pool_id: i32,
    question_type: String,
    count: usize,
    situation: Option<bool>,
    seed: Option<u64>,
    books: Option<Vec<generator::BookFilter>>,
    /// Number of sets to generate; set `i` uses `seed + i` so a seed reproduces every set.
    #[serde(default = "one_set")]
    sets: usize,
}

async fn generate(
    State(state): State<AppState>,
    client: ClientMeta,
    actor: Actor,
    Json(body): Json<GenerateBody>,
) -> ApiResult {
    if !(1..=MAX_SETS_PER_REQUEST).contains(&body.sets) {
        return Err(ApiError::bad_request(format!(
            "sets must be between 1 and {MAX_SETS_PER_REQUEST}"
        )));
    }
    let source_pool = authorize_pool_access(&state, &actor, body.pool_id).await?;
    let parsed = parsed_pool(&state, &source_pool).await?;

    enum GenerationSource {
        Cached(Arc<Vec<generator::QuestionRecord>>),
        Filtered(Vec<generator::QuestionRecord>),
    }
    let no_match = || ApiError::bad_request("No questions match selected books/chapters");

    let generation_source = match body.books.as_ref() {
        Some(filters) if !filters.is_empty() => {
            match generator::filter_by_books_cow(parsed.as_ref(), filters) {
                std::borrow::Cow::Borrowed(_) => GenerationSource::Cached(parsed.clone()),
                std::borrow::Cow::Owned(filtered) if filtered.is_empty() => return Err(no_match()),
                std::borrow::Cow::Owned(filtered) => GenerationSource::Filtered(filtered),
            }
        }
        _ if parsed.is_empty() => return Err(no_match()),
        _ => GenerationSource::Cached(parsed.clone()),
    };

    let req = generator::GenerateRequest {
        question_type: body.question_type,
        count: body.count,
        situation: body.situation,
        seed: body.seed,
    };
    let set_count = body.sets;
    let generation_permit = state
        .generation_slots
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Question generation is unavailable",
            )
        })?;
    let sets = tokio::task::spawn_blocking(move || {
        let _generation_permit = generation_permit;
        let pool = match &generation_source {
            GenerationSource::Cached(pool) => pool.as_slice(),
            GenerationSource::Filtered(pool) => pool.as_slice(),
        };
        (0..set_count)
            .map(|i| {
                let set_req = generator::GenerateRequest {
                    seed: req.seed.map(|seed| seed.wrapping_add(i as u64)),
                    ..req.clone()
                };
                generator::generate_questions(pool, &set_req)
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|_| ApiError::internal("Question generation failed"))?;

    audit::record(
        &state.pool,
        &client,
        actor.user_id,
        "questions.generated",
        "question_pool",
        source_pool.id,
        json!({
            "sets": sets.len(),
            "count": sets.iter().map(Vec::len).sum::<usize>(),
        }),
    )
    .await;

    ok(json!({
        "sets": sets,
        "pool": {
            "id": source_pool.id,
            "name": source_pool.name,
            "tier_id": source_pool.tier_id,
            "tier_name": source_pool.tier_name,
        },
    }))
}

// ── Admin: tiers ──

#[derive(Debug, Deserialize)]
struct TierBody {
    name: String,
    rank: i32,
}

fn tier_write_error(message: &'static str) -> impl FnOnce(sqlx::Error) -> ApiError {
    move |error| {
        if is_unique_violation(&error) {
            ApiError::conflict("A tier with that name or rank already exists")
        } else {
            db_error(message)(error)
        }
    }
}

async fn list_tiers(State(state): State<AppState>, _admin: Admin) -> ApiResult {
    let tiers = sqlx::query_as::<_, Tier>("SELECT id, name, rank FROM tiers ORDER BY rank ASC")
        .fetch_all(&state.pool)
        .await
        .map_err(db_error("Failed to load tiers"))?;
    ok(json!({"tiers": tiers}))
}

async fn create_tier(
    State(state): State<AppState>,
    client: ClientMeta,
    Admin(actor): Admin,
    Json(body): Json<TierBody>,
) -> ApiResult {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("Tier name must not be empty"));
    }
    authz::validate_tier_rank(None, body.rank).map_err(ApiError::bad_request)?;

    let tier = sqlx::query_as::<_, Tier>(
        "INSERT INTO tiers (name, rank) VALUES ($1, $2) RETURNING id, name, rank",
    )
    .bind(name)
    .bind(body.rank)
    .fetch_one(&state.pool)
    .await
    .map_err(tier_write_error("Failed to create tier"))?;

    audit::record(
        &state.pool,
        &client,
        Some(actor),
        "tier.created",
        "tier",
        tier.id,
        json!({"name": tier.name, "rank": tier.rank}),
    )
    .await;
    ok(json!({"tier": tier}))
}

async fn update_tier(
    State(state): State<AppState>,
    client: ClientMeta,
    Admin(actor): Admin,
    Path(id): Path<i32>,
    Json(body): Json<TierBody>,
) -> ApiResult {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("Tier name must not be empty"));
    }
    let current_rank = authz::tier_rank(&state.pool, id)
        .await
        .map_err(db_error("Failed to load tier"))?
        .ok_or_else(tier_not_found)?;
    authz::validate_tier_rank(Some(current_rank), body.rank).map_err(ApiError::bad_request)?;

    let tier = sqlx::query_as::<_, Tier>(
        "UPDATE tiers SET name = $1, rank = $2 WHERE id = $3 RETURNING id, name, rank",
    )
    .bind(name)
    .bind(body.rank)
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(tier_write_error("Failed to update tier"))?
    .ok_or_else(tier_not_found)?;

    audit::record(
        &state.pool,
        &client,
        Some(actor),
        "tier.updated",
        "tier",
        tier.id,
        json!({"name": tier.name, "rank": tier.rank}),
    )
    .await;
    ok(json!({"tier": tier}))
}

async fn delete_tier(
    State(state): State<AppState>,
    client: ClientMeta,
    Admin(actor): Admin,
    Path(id): Path<i32>,
) -> ApiResult {
    let rank = authz::tier_rank(&state.pool, id)
        .await
        .map_err(db_error("Failed to load tier"))?
        .ok_or_else(tier_not_found)?;
    if !authz::can_delete_tier(rank) {
        return Err(ApiError::bad_request("The public tier cannot be deleted"));
    }

    let tier_id = sqlx::query_scalar::<_, i32>("DELETE FROM tiers WHERE id = $1 RETURNING id")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| {
            if is_foreign_key_violation(&error) {
                ApiError::conflict("Tier is still assigned to users or pools")
            } else {
                db_error("Failed to delete tier")(error)
            }
        })?
        .ok_or_else(tier_not_found)?;

    audit::record(
        &state.pool,
        &client,
        Some(actor),
        "tier.deleted",
        "tier",
        tier_id,
        json!({}),
    )
    .await;
    ok(json!({"deleted": tier_id}))
}

// ── Admin: users ──

#[derive(Debug, Deserialize)]
struct CreateUserBody {
    username: String,
    password: String,
    tier_id: i32,
    is_admin: bool,
    #[serde(default)]
    can_upload_pools: bool,
}

#[derive(Debug, Deserialize)]
struct UpdateUserBody {
    username: String,
    tier_id: i32,
    is_admin: bool,
    #[serde(default)]
    can_upload_pools: bool,
    password: Option<String>,
}

fn user_write_error(message: &'static str) -> impl FnOnce(sqlx::Error) -> ApiError {
    move |error| {
        if is_unique_violation(&error) {
            ApiError::conflict("That username is already taken")
        } else if is_foreign_key_violation(&error) {
            ApiError::bad_request("Unknown tier")
        } else {
            db_error(message)(error)
        }
    }
}

fn user_json(user: &User) -> Value {
    json!({
        "id": user.id,
        "username": user.username,
        "tier_id": user.tier_id,
        "is_admin": user.is_admin,
        "can_upload_pools": user.can_upload_pools,
    })
}

async fn list_users(State(state): State<AppState>, _admin: Admin) -> ApiResult {
    let users = sqlx::query_as::<_, User>(
        "SELECT id, username, password_hash, is_admin, can_upload_pools, tier_id, created_at FROM users ORDER BY created_at DESC",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(db_error("Failed to load users"))?;

    let safe: Vec<_> = users
        .iter()
        .map(|u| {
            let mut entry = user_json(u);
            entry["created_at"] = json!(u.created_at);
            entry
        })
        .collect();
    ok(json!({"users": safe}))
}

async fn create_user(
    State(state): State<AppState>,
    client: ClientMeta,
    Admin(actor): Admin,
    Json(body): Json<CreateUserBody>,
) -> ApiResult {
    let username = body.username.trim();
    if username.is_empty() {
        return Err(ApiError::bad_request("Username must not be empty"));
    }
    auth::validate_password(&body.password).map_err(ApiError::bad_request)?;
    let password_hash = auth::hash_password_blocking(body.password)
        .await
        .map_err(|_| ApiError::internal("Failed to hash password"))?;

    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (username, password_hash, tier_id, is_admin, can_upload_pools)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id, username, password_hash, is_admin, can_upload_pools, tier_id, created_at",
    )
    .bind(username)
    .bind(password_hash)
    .bind(body.tier_id)
    .bind(body.is_admin)
    .bind(body.can_upload_pools)
    .fetch_one(&state.pool)
    .await
    .map_err(user_write_error("Failed to create user"))?;

    let summary = user_json(&user);
    audit::record(
        &state.pool,
        &client,
        Some(actor),
        "user.created",
        "user",
        user.id,
        summary.clone(),
    )
    .await;
    ok(summary)
}

async fn update_user(
    State(state): State<AppState>,
    client: ClientMeta,
    Admin(actor): Admin,
    jar: CookieJar,
    Path(id): Path<i32>,
    Json(body): Json<UpdateUserBody>,
) -> ApiResult {
    if actor == id && !body.is_admin {
        return Err(ApiError::bad_request(
            "You cannot remove your own admin role",
        ));
    }
    let username = body.username.trim();
    if username.is_empty() {
        return Err(ApiError::bad_request("Username must not be empty"));
    }

    // A blank password field in the admin UI means "leave unchanged".
    let password_hash = match body.password.as_deref().filter(|p| !p.trim().is_empty()) {
        Some(password) => {
            auth::validate_password(password).map_err(ApiError::bad_request)?;
            let hash = auth::hash_password_blocking(password.to_string())
                .await
                .map_err(|_| ApiError::internal("Failed to hash password"))?;
            Some(hash)
        }
        None => None,
    };
    let password_changed = password_hash.is_some();

    let user = update_user_record(
        &state,
        id,
        username,
        &body,
        password_hash,
        auth::session_token(&jar),
    )
    .await
    .map_err(user_write_error("Failed to update user"))?
    .ok_or_else(user_not_found)?;

    let summary = user_json(&user);
    let mut metadata = summary.clone();
    metadata["password_changed"] = json!(password_changed);
    audit::record(
        &state.pool,
        &client,
        Some(actor),
        "user.updated",
        "user",
        user.id,
        metadata,
    )
    .await;
    ok(summary)
}

/// Applies a user edit. A password change also signs the user out of every other session,
/// in the same transaction, so a leaked or shared password stops working immediately.
async fn update_user_record(
    state: &AppState,
    id: i32,
    username: &str,
    body: &UpdateUserBody,
    password_hash: Option<String>,
    current_session: Option<uuid::Uuid>,
) -> Result<Option<User>, sqlx::Error> {
    let mut tx = state.pool.begin().await?;
    let password_changed = password_hash.is_some();

    let updated = sqlx::query_as::<_, User>(
        "UPDATE users
         SET username = $1, tier_id = $2, is_admin = $3, can_upload_pools = $4,
             password_hash = COALESCE($5, password_hash)
         WHERE id = $6
         RETURNING id, username, password_hash, is_admin, can_upload_pools, tier_id, created_at",
    )
    .bind(username)
    .bind(body.tier_id)
    .bind(body.is_admin)
    .bind(body.can_upload_pools)
    .bind(password_hash)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;

    if updated.is_some() && password_changed {
        auth::revoke_user_sessions(&mut *tx, id, current_session).await?;
    }

    tx.commit().await?;
    Ok(updated)
}

async fn delete_user(
    State(state): State<AppState>,
    client: ClientMeta,
    Admin(actor): Admin,
    Path(id): Path<i32>,
) -> ApiResult {
    if actor == id {
        return Err(ApiError::bad_request("You cannot delete your own account"));
    }

    let user_id = sqlx::query_scalar::<_, i32>("DELETE FROM users WHERE id = $1 RETURNING id")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(db_error("Failed to delete user"))?
        .ok_or_else(user_not_found)?;

    audit::record(
        &state.pool,
        &client,
        Some(actor),
        "user.deleted",
        "user",
        user_id,
        json!({}),
    )
    .await;
    ok(json!({"deleted": user_id}))
}

// ── Admin: audit log ──

#[derive(Debug, Deserialize)]
struct AuditQuery {
    before: Option<i64>,
    action: Option<String>,
    limit: Option<i64>,
}

async fn list_audit(
    State(state): State<AppState>,
    _admin: Admin,
    Query(query): Query<AuditQuery>,
) -> ApiResult {
    let limit = query
        .limit
        .unwrap_or(DEFAULT_AUDIT_PAGE)
        .clamp(1, MAX_AUDIT_PAGE);
    let action = query
        .action
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty());

    let entries = audit::list(&state.pool, query.before, action, limit)
        .await
        .map_err(db_error("Failed to load audit log"))?;
    let next_before = if entries.len() as i64 == limit {
        entries.last().map(|e| e.id)
    } else {
        None
    };
    ok(json!({"entries": entries, "next_before": next_before}))
}
