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

fn upload_request(session: &str, name: &str, tier_id: i32, csv: &str) -> Request<Body> {
    let boundary = "qsets-test-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\n{name}\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"tier_id\"\r\n\r\n{tier_id}\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"pool.csv\"\r\n\
         Content-Type: text/csv\r\n\r\n{csv}\r\n--{boundary}--\r\n"
    );
    request(Method::POST, "/api/pools", Some(session))
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap()
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
    assert_eq!(body["questions"].as_array().unwrap().len(), 2);
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
    // The throttle is process-wide, so this username must be unique across the test binary.
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
