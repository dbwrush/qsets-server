//! End-to-end checks of the HTTP layer against a real PostgreSQL database.
//!
//! Each `#[sqlx::test]` gets a fresh, migrated database created from the server named in
//! `DATABASE_URL` (read from the environment or `.env`), so the role needs CREATEDB.

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use qsets_server::{services::auth, state::AppState};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;

const CSRF: &str = "test-csrf-token";
const PASSWORD: &str = "correct horse battery";
const SAMPLE_CSV: &str = "Question,Type,Reference,Answer\n\
    Who was in the beginning?,General,John 1:1,The Word\n\
    What was the Word with?,General,John 1:2,God\n\
    What was made through him?,General,John 1:3,All things\n";

fn app(pool: &PgPool) -> Router {
    app_with_proxy(pool, false)
}

fn app_with_proxy(pool: &PgPool, trust_proxy: bool) -> Router {
    qsets_server::app(AppState::new(pool.clone(), 2, trust_proxy), "static")
}

/// Builds a request carrying a valid CSRF cookie/header pair and, optionally, a session.
fn request(method: Method, uri: &str, session: Option<&str>) -> axum::http::request::Builder {
    let mut cookie = format!("qsets_csrf={CSRF}");
    if let Some(token) = session {
        cookie.push_str(&format!("; qsets_session={token}"));
    }
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", CSRF)
}

fn json_request(method: Method, uri: &str, session: Option<&str>, body: Value) -> Request<Body> {
    request(method, uri, session)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get(uri: &str, session: Option<&str>) -> Request<Body> {
    request(Method::GET, uri, session)
        .body(Body::empty())
        .unwrap()
}

const BOUNDARY: &str = "qsets-test-boundary";

fn multipart_file(csv: &str) -> String {
    format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"pool.csv\"\r\n\
         Content-Type: text/csv\r\n\r\n{csv}\r\n--{BOUNDARY}--\r\n"
    )
}

fn multipart_request(method: Method, uri: &str, session: &str, body: String) -> Request<Body> {
    request(method, uri, Some(session))
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap()
}

fn upload_request(session: &str, name: &str, tier_id: i32, csv: &str) -> Request<Body> {
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\n{name}\r\n\
         --{BOUNDARY}\r\nContent-Disposition: form-data; name=\"tier_id\"\r\n\r\n{tier_id}\r\n{}",
        multipart_file(csv)
    );
    multipart_request(Method::POST, "/api/pools", session, body)
}

fn replace_request(session: &str, pool_id: i32, csv: &str) -> Request<Body> {
    multipart_request(
        Method::PUT,
        &format!("/api/pools/{pool_id}"),
        session,
        multipart_file(csv),
    )
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

async fn create_tier(pool: &PgPool, name: &str, rank: i32) -> i32 {
    sqlx::query_scalar("INSERT INTO tiers (name, rank) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(rank)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn public_tier(pool: &PgPool) -> i32 {
    sqlx::query_scalar("SELECT id FROM tiers WHERE rank = 0")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn create_user(
    pool: &PgPool,
    username: &str,
    tier_id: i32,
    is_admin: bool,
    can_upload: bool,
) -> i32 {
    sqlx::query_scalar(
        "INSERT INTO users (username, password_hash, tier_id, is_admin, can_upload_pools)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(username)
    .bind(auth::hash_password(PASSWORD).unwrap())
    .bind(tier_id)
    .bind(is_admin)
    .bind(can_upload)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn session(pool: &PgPool, user_id: i32) -> String {
    auth::create_session(pool, user_id).await.unwrap()
}

async fn create_pool(pool: &PgPool, name: &str, tier_id: i32) -> i32 {
    sqlx::query_scalar(
        "INSERT INTO question_pools (name, tier_id, csv_text) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(name)
    .bind(tier_id)
    .bind(SAMPLE_CSV)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn pool_names(body: &Value) -> Vec<String> {
    body["pools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect()
}

#[sqlx::test]
async fn pools_above_the_callers_tier_are_hidden_and_forbidden(pool: PgPool) {
    let app = app(&pool);
    let public = public_tier(&pool).await;
    let district = create_tier(&pool, "district", 1).await;
    let regional = create_tier(&pool, "regional", 2).await;
    create_pool(&pool, "public-pool", public).await;
    let regional_pool = create_pool(&pool, "regional-pool", regional).await;

    let district_user = create_user(&pool, "district-user", district, false, false).await;
    let regional_user = create_user(&pool, "regional-user", regional, false, false).await;
    let district_session = session(&pool, district_user).await;
    let regional_session = session(&pool, regional_user).await;
    let books = format!("/api/pools/{regional_pool}/books");

    let (status, body) = send(&app, get("/api/pools", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pool_names(&body), ["public-pool"]);

    let (status, body) = send(&app, get("/api/pools", Some(&district_session))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!pool_names(&body).contains(&"regional-pool".to_string()));

    let (status, _) = send(&app, get(&books, None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&app, get(&books, Some(&district_session))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let generate = json!({"pool_id": regional_pool, "question_type": "all", "count": 2});
    let (status, _) = send(
        &app,
        json_request(
            Method::POST,
            "/api/generate",
            Some(&district_session),
            generate.clone(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(&app, get(&books, Some(&regional_session))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(
        &app,
        json_request(
            Method::POST,
            "/api/generate",
            Some(&regional_session),
            generate,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sets"][0].as_array().unwrap().len(), 2);
}

#[sqlx::test]
async fn uploaders_cannot_assign_a_tier_above_their_own(pool: PgPool) {
    let app = app(&pool);
    let district = create_tier(&pool, "district", 1).await;
    let regional = create_tier(&pool, "regional", 2).await;
    let uploader = create_user(&pool, "uploader", district, false, true).await;
    let reader = create_user(&pool, "reader", regional, false, false).await;
    let uploader_session = session(&pool, uploader).await;
    let reader_session = session(&pool, reader).await;

    let (status, _) = send(
        &app,
        upload_request(&uploader_session, "too-high", regional, SAMPLE_CSV),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &app,
        upload_request(&reader_session, "no-permission", district, SAMPLE_CSV),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = send(
        &app,
        upload_request(&uploader_session, "district-pool", district, SAMPLE_CSV),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["validation"]["valid_count"], 3);

    let (status, _) = send(
        &app,
        upload_request(&uploader_session, "district-pool", district, SAMPLE_CSV),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[sqlx::test]
async fn mutating_requests_require_a_matching_csrf_token(pool: PgPool) {
    let app = app(&pool);
    let pool_id = create_pool(&pool, "public-pool", public_tier(&pool).await).await;
    let body = json!({"pool_id": pool_id, "question_type": "all", "count": 1}).to_string();

    let missing_header = Request::builder()
        .method(Method::POST)
        .uri("/api/generate")
        .header(header::COOKIE, format!("qsets_csrf={CSRF}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.clone()))
        .unwrap();
    let (status, _) = send(&app, missing_header).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let mismatched = Request::builder()
        .method(Method::POST)
        .uri("/api/generate")
        .header(header::COOKIE, format!("qsets_csrf={CSRF}"))
        .header("x-csrf-token", "something-else")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.clone()))
        .unwrap();
    let (status, _) = send(&app, mismatched).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &app,
        json_request(
            Method::POST,
            "/api/generate",
            None,
            serde_json::from_str(&body).unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

fn login_attempt(username: &str, password: &str, forwarded_for: &str) -> Request<Body> {
    request(Method::POST, "/api/login", None)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-forwarded-for", forwarded_for)
        .body(Body::from(
            json!({"username": username, "password": password}).to_string(),
        ))
        .unwrap()
}

#[sqlx::test]
async fn login_throttle_cannot_be_bypassed_with_forwarded_headers(pool: PgPool) {
    let username = "throttle-untrusted";
    let app = app(&pool);
    create_user(&pool, username, public_tier(&pool).await, false, false).await;

    for attempt in 0..10 {
        let spoofed = format!("198.51.100.{attempt}");
        let (status, _) = send(&app, login_attempt(username, "wrong-password", &spoofed)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    let (status, _) = send(&app, login_attempt(username, PASSWORD, "198.51.100.200")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[sqlx::test]
async fn trusted_proxy_throttles_per_forwarded_client(pool: PgPool) {
    let username = "throttle-trusted";
    let app = app_with_proxy(&pool, true);
    create_user(&pool, username, public_tier(&pool).await, false, false).await;

    for _ in 0..10 {
        let (status, _) = send(&app, login_attempt(username, "wrong", "203.0.113.7")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, _) = send(&app, login_attempt(username, PASSWORD, "203.0.113.7")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    // A different client behind the same proxy is not locked out.
    let (status, _) = send(&app, login_attempt(username, PASSWORD, "203.0.113.8")).await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test]
async fn password_change_signs_the_user_out_of_other_sessions(pool: PgPool) {
    let app = app(&pool);
    let public = public_tier(&pool).await;
    let admin = create_user(&pool, "admin", public, true, true).await;
    let member = create_user(&pool, "member", public, false, false).await;
    let admin_session = session(&pool, admin).await;
    let admin_other_session = session(&pool, admin).await;
    let member_session = session(&pool, member).await;

    let update = |id: i32, username: &str, is_admin: bool, password: Option<&str>| {
        let body = json!({
            "username": username,
            "tier_id": public,
            "is_admin": is_admin,
            "can_upload_pools": is_admin,
            "password": password,
        });
        (format!("/api/admin/users/{id}"), body)
    };

    // Editing without a password leaves sessions alone.
    let (uri, body) = update(member, "member", false, None);
    let (status, _) = send(
        &app,
        json_request(Method::PUT, &uri, Some(&admin_session), body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&app, get("/api/me", Some(&member_session))).await;
    assert_eq!(status, StatusCode::OK);

    let (uri, body) = update(member, "member", false, Some("short"));
    let (status, _) = send(
        &app,
        json_request(Method::PUT, &uri, Some(&admin_session), body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (uri, body) = update(member, "member", false, Some("a brand new password"));
    let (status, _) = send(
        &app,
        json_request(Method::PUT, &uri, Some(&admin_session), body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&app, get("/api/me", Some(&member_session))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Changing your own password keeps the session that made the change.
    let (uri, body) = update(admin, "admin", true, Some("another new password"));
    let (status, _) = send(
        &app,
        json_request(Method::PUT, &uri, Some(&admin_session), body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&app, get("/api/me", Some(&admin_session))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&app, get("/api/me", Some(&admin_other_session))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn user_creation_enforces_password_policy_and_unique_names(pool: PgPool) {
    let app = app(&pool);
    let public = public_tier(&pool).await;
    let admin = create_user(&pool, "admin", public, true, true).await;
    let admin_session = session(&pool, admin).await;
    let new_user = |password: &str| json!({"username": "newbie", "password": password, "tier_id": public, "is_admin": false});

    let (status, _) = send(
        &app,
        json_request(
            Method::POST,
            "/api/admin/users",
            Some(&admin_session),
            new_user(""),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &app,
        json_request(
            Method::POST,
            "/api/admin/users",
            Some(&admin_session),
            new_user("long enough"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = send(
        &app,
        json_request(
            Method::POST,
            "/api/admin/users",
            Some(&admin_session),
            new_user("long enough"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[sqlx::test]
async fn public_tier_cannot_be_moved_or_deleted(pool: PgPool) {
    let app = app(&pool);
    let public = public_tier(&pool).await;
    let admin = create_user(&pool, "admin", public, true, true).await;
    let admin_session = session(&pool, admin).await;
    let tier_uri = format!("/api/admin/tiers/{public}");

    let (status, _) = send(
        &app,
        json_request(
            Method::PUT,
            &tier_uri,
            Some(&admin_session),
            json!({"name": "public", "rank": 5}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &app,
        json_request(
            Method::PUT,
            &tier_uri,
            Some(&admin_session),
            json!({"name": "everyone", "rank": 0}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = send(
        &app,
        request(Method::DELETE, &tier_uri, Some(&admin_session))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &app,
        json_request(
            Method::POST,
            "/api/admin/tiers",
            Some(&admin_session),
            json!({"name": "below-public", "rank": -1}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &app,
        json_request(
            Method::POST,
            "/api/admin/tiers",
            Some(&admin_session),
            json!({"name": "duplicate-rank", "rank": 0}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[sqlx::test]
async fn responses_carry_security_headers(pool: PgPool) {
    let response = app(&pool).oneshot(get("/login", None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(csp.contains("script-src 'self'"));
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
}

fn generate_request(pool_id: i32, extra: Value) -> Request<Body> {
    let mut body = json!({"pool_id": pool_id, "question_type": "all", "count": 3});
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    json_request(Method::POST, "/api/generate", None, body)
}

fn questions(set: &Value) -> Vec<String> {
    set.as_array()
        .unwrap()
        .iter()
        .map(|q| q["question"].as_str().unwrap().to_string())
        .collect()
}

#[sqlx::test]
async fn csrf_is_checked_before_the_body_is_parsed(pool: PgPool) {
    let forged = Request::builder()
        .method(Method::POST)
        .uri("/api/admin/tiers")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("not json"))
        .unwrap();
    let (status, body) = send(&app(&pool), forged).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "CSRF validation failed");
}

#[sqlx::test]
async fn login_throttle_stops_password_spraying_from_one_client(pool: PgPool) {
    let app = app(&pool);
    create_user(&pool, "victim", public_tier(&pool).await, false, false).await;

    for n in 0..50 {
        let (status, _) = send(
            &app,
            login_attempt(&format!("guess-{n}"), "wrong", "198.51.100.1"),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, _) = send(&app, login_attempt("victim", PASSWORD, "198.51.100.1")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[sqlx::test]
async fn unknown_and_known_users_get_the_same_login_failure(pool: PgPool) {
    let app = app(&pool);
    create_user(&pool, "known", public_tier(&pool).await, false, false).await;

    let (status, unknown) = send(&app, login_attempt("nobody", "wrong", "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, known) = send(&app, login_attempt("known", "wrong", "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown, known);
}

#[sqlx::test]
async fn users_can_change_their_own_password(pool: PgPool) {
    let app = app(&pool);
    let user = create_user(&pool, "member", public_tier(&pool).await, false, false).await;
    let current = session(&pool, user).await;
    let other = session(&pool, user).await;
    let change = |current_password: &str, new_password: &str| {
        json_request(
            Method::POST,
            "/api/me/password",
            Some(&current),
            json!({"current_password": current_password, "new_password": new_password}),
        )
    };

    let (status, _) = send(
        &app,
        json_request(
            Method::POST,
            "/api/me/password",
            None,
            json!({"current_password": PASSWORD, "new_password": "whatever long"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(&app, change("not my password", "a fresh password")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&app, change(PASSWORD, "short")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = send(&app, change(PASSWORD, "a fresh password")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = send(&app, get("/api/me", Some(&current))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&app, get("/api/me", Some(&other))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(&app, login_attempt("member", PASSWORD, "")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(&app, login_attempt("member", "a fresh password", "")).await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test]
async fn wrong_current_passwords_are_throttled(pool: PgPool) {
    let app = app(&pool);
    let user = create_user(&pool, "member", public_tier(&pool).await, false, false).await;
    let token = session(&pool, user).await;
    let attempt = |current_password: &str| {
        json_request(
            Method::POST,
            "/api/me/password",
            Some(&token),
            json!({"current_password": current_password, "new_password": "a fresh password"}),
        )
    };

    for _ in 0..10 {
        let (status, _) = send(&app, attempt("wrong")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    let (status, _) = send(&app, attempt(PASSWORD)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[sqlx::test]
async fn owners_can_replace_a_pool_and_generation_sees_the_new_questions(pool: PgPool) {
    let app = app(&pool);
    let public = public_tier(&pool).await;
    let owner = create_user(&pool, "owner", public, false, true).await;
    let other = create_user(&pool, "other-uploader", public, false, true).await;
    let owner_session = session(&pool, owner).await;
    let other_session = session(&pool, other).await;

    let (status, body) = send(
        &app,
        upload_request(&owner_session, "owned", public, SAMPLE_CSV),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let pool_id = body["id"].as_i64().unwrap() as i32;

    // Generate first so the old CSV is cached.
    let (status, body) = send(&app, generate_request(pool_id, json!({}))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(questions(&body["sets"][0]).contains(&"Who was in the beginning?".to_string()));

    let replacement = "Question,Type,Reference,Answer\n\
        Who sent John?,General,John 1:6,God\n";
    let (status, _) = send(&app, replace_request(&other_session, pool_id, replacement)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&app, replace_request(&owner_session, pool_id, "")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = send(&app, replace_request(&owner_session, pool_id, replacement)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["validation"]["valid_count"], 1);

    let (status, body) = send(&app, generate_request(pool_id, json!({}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(questions(&body["sets"][0]), ["Who sent John?"]);

    let (_, body) = send(&app, get("/api/pools", Some(&owner_session))).await;
    assert_eq!(body["pools"][0]["can_manage"], true);
    let (_, body) = send(&app, get("/api/pools", Some(&other_session))).await;
    assert_eq!(body["pools"][0]["can_manage"], false);
}

#[sqlx::test]
async fn generate_returns_several_reproducible_sets_in_one_request(pool: PgPool) {
    let app = app(&pool);
    let pool_id = create_pool(&pool, "public-pool", public_tier(&pool).await).await;

    let (status, first) = send(
        &app,
        generate_request(pool_id, json!({"sets": 4, "seed": 7})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["sets"].as_array().unwrap().len(), 4);
    let (_, again) = send(
        &app,
        generate_request(pool_id, json!({"sets": 4, "seed": 7})),
    )
    .await;
    assert_eq!(first["sets"], again["sets"]);

    // Set i uses seed + i, so a single-set request with seed 8 matches set 1 above.
    let (_, shifted) = send(&app, generate_request(pool_id, json!({"seed": 8}))).await;
    assert_eq!(shifted["sets"][0], first["sets"][1]);

    for sets in [0, 21] {
        let (status, _) = send(&app, generate_request(pool_id, json!({"sets": sets}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}

#[sqlx::test]
async fn admins_can_page_and_filter_the_audit_log(pool: PgPool) {
    let app = app(&pool);
    let public = public_tier(&pool).await;
    let admin = create_user(&pool, "admin", public, true, true).await;
    let member = create_user(&pool, "member", public, false, false).await;
    let admin_session = session(&pool, admin).await;
    let member_session = session(&pool, member).await;

    for n in 0..3 {
        send(&app, login_attempt("member", &format!("wrong-{n}"), "")).await;
    }
    let pool_id = create_pool(&pool, "public-pool", public).await;
    send(&app, generate_request(pool_id, json!({}))).await;

    let (status, _) = send(&app, get("/api/admin/audit", Some(&member_session))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&app, get("/api/admin/audit", None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = send(
        &app,
        get(
            "/api/admin/audit?action=login.&limit=2",
            Some(&admin_session),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let page = body["entries"].as_array().unwrap();
    assert_eq!(page.len(), 2);
    assert!(page.iter().all(|e| e["action"] == "login.failed"));
    assert_eq!(page[0]["actor_username"], "member");
    let before = body["next_before"].as_i64().unwrap();

    let (_, body) = send(
        &app,
        get(
            &format!("/api/admin/audit?action=login.&limit=2&before={before}"),
            Some(&admin_session),
        ),
    )
    .await;
    assert_eq!(body["entries"].as_array().unwrap().len(), 1);
    assert!(body["next_before"].is_null());

    let (_, body) = send(&app, get("/api/admin/audit", Some(&admin_session))).await;
    assert_eq!(body["entries"][0]["action"], "questions.generated");
}

#[sqlx::test]
async fn admin_page_redirects_visitors_and_forbids_plain_users(pool: PgPool) {
    let app = app(&pool);
    let member = create_user(&pool, "member", public_tier(&pool).await, false, false).await;
    let member_session = session(&pool, member).await;

    for page in ["/admin", "/account"] {
        let response = app.clone().oneshot(get(page, None)).await.unwrap();
        assert!(response.status().is_redirection(), "{page}");
        assert_eq!(response.headers()[header::LOCATION], "/login");
    }

    let response = app
        .clone()
        .oneshot(get("/admin", Some(&member_session)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = app
        .clone()
        .oneshot(get("/account", Some(&member_session)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[sqlx::test]
async fn tiers_in_use_cannot_be_deleted(pool: PgPool) {
    let app = app(&pool);
    let admin = create_user(&pool, "admin", public_tier(&pool).await, true, true).await;
    let admin_session = session(&pool, admin).await;
    let district = create_tier(&pool, "district", 1).await;
    create_user(&pool, "district-user", district, false, false).await;

    let (status, body) = send(
        &app,
        request(
            Method::DELETE,
            &format!("/api/admin/tiers/{district}"),
            Some(&admin_session),
        )
        .body(Body::empty())
        .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
}
