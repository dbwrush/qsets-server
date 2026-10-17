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
async fn private_pools_are_hidden_from_everyone_except_owner_and_admin(pool: PgPool) {
    let app = app(&pool);
    let public = public_tier(&pool).await;
    let private: i32 = sqlx::query_scalar("SELECT id FROM tiers WHERE name = 'private'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let high = create_tier(&pool, "high", 50).await;
    let owner = create_user(&pool, "pool-owner", public, false, true).await;
    let other = create_user(&pool, "other-uploader", high, false, true).await;
    let admin = create_user(&pool, "admin", public, true, false).await;
    let reader = create_user(&pool, "reader", public, false, false).await;
    let owner_session = session(&pool, owner).await;
    let other_session = session(&pool, other).await;
    let admin_session = session(&pool, admin).await;
    let reader_session = session(&pool, reader).await;

    let (status, uploaded) = send(
        &app,
        upload_request(&owner_session, "owner-private", private, SAMPLE_CSV),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{uploaded}");
    let id = uploaded["id"].as_i64().unwrap();
    for caller in [
        None,
        Some(other_session.as_str()),
        Some(reader_session.as_str()),
    ] {
        let (status, body) = send(&app, get("/api/pools", caller)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!pool_names(&body).contains(&"owner-private".to_string()));
        let (status, _) = send(&app, get(&format!("/api/pools/{id}/books"), caller)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        for uri in ["/api/generate", "/api/generate/validate"] {
            let (status, _) = send(
                &app,
                json_request(
                    Method::POST,
                    uri,
                    caller,
                    json!({
                        "pool_id": id, "question_type": "custom", "count": 1,
                        "custom_counts": {"General": 1},
                        "books": [{"name": "John", "start_chapter": 1, "end_chapter": 1}],
                    }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        }
    }
    for caller in [&owner_session, &admin_session] {
        let (status, body) = send(&app, get("/api/pools", Some(caller))).await;
        assert_eq!(status, StatusCode::OK);
        let found = body["pools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap();
        assert_eq!(found["tier_name"], "private");
        assert_eq!(found["created_by"], owner);
        assert_eq!(found["created_by_username"], "pool-owner");
        assert_eq!(found["can_manage"], true);
        assert!(found.get("csv_text").is_none());
        let (status, _) = send(&app, get(&format!("/api/pools/{id}/books"), Some(caller))).await;
        assert_eq!(status, StatusCode::OK);
        for uri in ["/api/generate", "/api/generate/validate"] {
            let (status, _) = send(
                &app,
                json_request(
                    Method::POST,
                    uri,
                    Some(caller),
                    json!({
                        "pool_id": id, "question_type": "custom", "count": 1,
                        "custom_counts": {"General": 1},
                        "books": [{"name": "John", "start_chapter": 1, "end_chapter": 1}],
                    }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{uri}");
        }
    }
    let (status, _) = send(&app, replace_request(&other_session, id as i32, SAMPLE_CSV)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &app,
        request(
            Method::DELETE,
            &format!("/api/pools/{id}"),
            Some(&other_session),
        )
        .body(Body::empty())
        .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&app, replace_request(&owner_session, id as i32, SAMPLE_CSV)).await;
    assert_eq!(status, StatusCode::OK);

    sqlx::query("UPDATE users SET can_upload_pools = FALSE WHERE id = $1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    let (status, _) = send(
        &app,
        get(&format!("/api/pools/{id}/books"), Some(&owner_session)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&app, replace_request(&owner_session, id as i32, SAMPLE_CSV)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &app,
        upload_request(&reader_session, "not-allowed", private, SAMPLE_CSV),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &app,
        upload_request(&owner_session, "revoked", private, SAMPLE_CSV),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = send(&app, get("/api/pools", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["pools"].as_array().unwrap().is_empty());
    let (_, body) = send(&app, get("/api/pools", Some(&admin_session))).await;
    assert_eq!(body["pools"][0]["created_by_username"], Value::Null);
    let (status, _) = send(&app, replace_request(&admin_session, id as i32, SAMPLE_CSV)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &app,
        request(
            Method::DELETE,
            &format!("/api/pools/{id}"),
            Some(&admin_session),
        )
        .body(Body::empty())
        .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test]
async fn private_tier_is_pool_only_and_cannot_be_changed(pool: PgPool) {
    let app = app(&pool);
    let public = public_tier(&pool).await;
    let private: i32 = sqlx::query_scalar("SELECT id FROM tiers WHERE name = 'private'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let admin = create_user(&pool, "admin", public, true, true).await;
    let uploader = create_user(&pool, "uploader", public, false, true).await;
    let admin_session = session(&pool, admin).await;
    let uploader_session = session(&pool, uploader).await;
    let (_, body) = send(&app, get("/api/tiers", Some(&uploader_session))).await;
    assert!(body["tiers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["id"] == private));
    let (_, body) = send(&app, get("/api/tiers", None)).await;
    assert!(!body["tiers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["id"] == private));
    for (method, uri, body) in [
        (
            Method::PUT,
            format!("/api/admin/tiers/{private}"),
            json!({"name": "shared", "rank": 1}),
        ),
        (
            Method::DELETE,
            format!("/api/admin/tiers/{private}"),
            json!({}),
        ),
        (
            Method::POST,
            "/api/admin/tiers".to_string(),
            json!({"name": "PRIVATE", "rank": 1}),
        ),
        (
            Method::PUT,
            format!("/api/admin/tiers/{public}"),
            json!({"name": "Private", "rank": 0}),
        ),
        (
            Method::POST,
            "/api/admin/users".to_string(),
            json!({
                "username": "bad-user", "password": PASSWORD, "tier_id": private, "is_admin": false,
            }),
        ),
        (
            Method::PUT,
            format!("/api/admin/users/{uploader}"),
            json!({
                "username": "uploader", "tier_id": private, "is_admin": false,
            }),
        ),
    ] {
        let (status, body) =
            send(&app, json_request(method, &uri, Some(&admin_session), body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
    }
}

#[sqlx::test]
async fn uploaders_have_a_non_admin_pool_management_page(pool: PgPool) {
    let app = app(&pool);
    let public = public_tier(&pool).await;
    let uploader = create_user(&pool, "uploader", public, false, true).await;
    let reader = create_user(&pool, "reader", public, false, false).await;
    let uploader_session = session(&pool, uploader).await;
    let reader_session = session(&pool, reader).await;
    let header = header_html(&app, "/", Some(&uploader_session)).await;
    assert!(header.contains(r#"href="/pools""#));
    assert!(!header.contains(r#"href="/admin""#));
    let response = app
        .clone()
        .oneshot(get("/pools", Some(&uploader_session)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("My pools"));
    assert!(html.contains(r#"id="upload-form""#));
    assert!(!html.contains(r#"id="user-form""#));
    assert!(!html.contains(r#"id="tier-form""#));
    let (status, _) = send(&app, get("/pools", Some(&reader_session))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let response = app.oneshot(get("/pools", None)).await.unwrap();
    assert_eq!(response.headers()[header::LOCATION], "/login");
}

#[sqlx::test]
async fn study_returns_filtered_questions_without_saving_a_session(pool: PgPool) {
    let app = app(&pool);
    let public = public_tier(&pool).await;
    let pool_id = create_pool(&pool, "study-pool", public).await;
    sqlx::query("UPDATE question_pools SET csv_text = csv_text || $1 WHERE id = $2")
        .bind(
            "Other chapter,General,John 2:1,Other answer\nOther type,Quote,John 1:4,Quote answer\n",
        )
        .bind(pool_id)
        .execute(&pool)
        .await
        .unwrap();
    let body = json!({
        "pool_id": pool_id,
        "books": [{"name": "John", "start_chapter": 1, "end_chapter": 1}],
        "types": ["General"], "count": 2,
    });
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/study/questions",
            None,
            body.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let data: Value = serde_json::from_slice(&bytes).unwrap();
    let selected = data["questions"].as_array().unwrap();
    assert_eq!(selected.len(), 2);
    assert_ne!(selected[0]["question"], selected[1]["question"]);
    assert!(selected
        .iter()
        .all(|q| q["type"] == "General" && q["answer"].is_string()));
    for count in [0, 999] {
        let mut body = body.clone();
        body["count"] = json!(count);
        let (status, data) = send(
            &app,
            json_request(Method::POST, "/api/study/questions", None, body),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(data["questions"].as_array().unwrap().len(), 3);
    }
    for change in [
        json!({"types": []}),
        json!({"types": ["Unknown"]}),
        json!({"types": ["Verse"]}),
        json!({"books": []}),
        json!({"books": [{"name": "John", "start_chapter": 3, "end_chapter": 3}]}),
    ] {
        let mut invalid = body.clone();
        invalid
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        let (status, _) = send(
            &app,
            json_request(Method::POST, "/api/study/questions", None, invalid),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_logs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(events, 0);
    let (status, _) = send(&app, get("/study", None)).await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test]
async fn study_enforces_private_and_shared_pool_access(pool: PgPool) {
    let app = app(&pool);
    let public = public_tier(&pool).await;
    let high = create_tier(&pool, "high", 2).await;
    let private: i32 = sqlx::query_scalar("SELECT id FROM tiers WHERE rank = -1")
        .fetch_one(&pool)
        .await
        .unwrap();
    let owner = create_user(&pool, "owner", public, false, true).await;
    let other = create_user(&pool, "other", high, false, true).await;
    let admin = create_user(&pool, "admin", public, true, false).await;
    let owner_session = session(&pool, owner).await;
    let other_session = session(&pool, other).await;
    let admin_session = session(&pool, admin).await;
    let private_pool = create_pool(&pool, "private-study", private).await;
    sqlx::query("UPDATE question_pools SET created_by = $1 WHERE id = $2")
        .bind(owner)
        .bind(private_pool)
        .execute(&pool)
        .await
        .unwrap();
    let high_pool = create_pool(&pool, "high-study", high).await;
    for (pool_id, caller, expected) in [
        (private_pool, None, StatusCode::FORBIDDEN),
        (
            private_pool,
            Some(other_session.as_str()),
            StatusCode::FORBIDDEN,
        ),
        (private_pool, Some(owner_session.as_str()), StatusCode::OK),
        (private_pool, Some(admin_session.as_str()), StatusCode::OK),
        (
            high_pool,
            Some(owner_session.as_str()),
            StatusCode::FORBIDDEN,
        ),
        (high_pool, Some(other_session.as_str()), StatusCode::OK),
    ] {
        let (status, _) = send(
            &app,
            json_request(
                Method::POST,
                "/api/study/questions",
                caller,
                json!({
                    "pool_id": pool_id, "types": ["General"], "count": 1,
                    "books": [{"name": "John", "start_chapter": 1, "end_chapter": 1}],
                }),
            ),
        )
        .await;
        assert_eq!(status, expected);
    }
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
async fn standard_generation_reports_filtered_shortages_without_returning_sets(pool: PgPool) {
    let app = app(&pool);
    let pool_id = create_pool(&pool, "public-pool", public_tier(&pool).await).await;
    let mut rows = vec!["Question,Type,Reference,Answer".to_string()];
    for chapter in 1..=8 {
        for (qtype, count) in [
            ("Situation", 1),
            ("In-What-Book-and-Chapter", 1),
            ("Quote", 1),
            ("Reference", 1),
            ("Verse", 1),
            ("Context", 1),
            ("According-To", 2),
            ("General", 3),
        ] {
            if chapter >= 4 && ["Quote", "Reference", "Verse"].contains(&qtype) {
                continue;
            }
            for index in 0..count {
                rows.push(format!(
                    "Question {chapter} {qtype} {index},{qtype},Luke {chapter}:1,Answer"
                ));
            }
        }
    }
    sqlx::query("UPDATE question_pools SET csv_text = $1, updated_at = NOW() WHERE id = $2")
        .bind(rows.join("\n"))
        .bind(pool_id)
        .execute(&pool)
        .await
        .unwrap();

    for situation in [true, false] {
        let (status, body) = send(
            &app,
            generate_request(
                pool_id,
                json!({
                    "question_type": "standard",
                    "count": 20,
                    "situation": situation,
                    "sets": 3,
                    "books": [{"name": "Luke", "start_chapter": 4, "end_chapter": 8}],
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let error = body["error"].as_str().unwrap();
        for qtype in ["Quote", "Reference", "Verse"] {
            assert!(
                error.contains(&format!("{qtype} (need 1, available 0)")),
                "{error}"
            );
        }
        assert!(body.get("sets").is_none());
    }
    let generated: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_logs WHERE action = 'questions.generated'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(generated, 0);

    let (status, body) = send(
        &app,
        generate_request(
            pool_id,
            json!({"question_type": "standard", "count": 20, "seed": 7}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sets"][0].as_array().unwrap().len(), 20);
}

#[sqlx::test]
async fn custom_generation_returns_exact_reproducible_sets_and_reports_errors(pool: PgPool) {
    let app = app(&pool);
    let pool_id = create_pool(&pool, "public-pool", public_tier(&pool).await).await;
    let options = json!({
        "question_type": "custom",
        "custom_counts": {"General": 2},
        "sets": 3,
        "seed": 42,
        "books": [{"name": "John", "start_chapter": 1, "end_chapter": 1}],
    });
    let (status, first) = send(&app, generate_request(pool_id, options.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["sets"].as_array().unwrap().len(), 3);
    for set in first["sets"].as_array().unwrap() {
        assert_eq!(set.as_array().unwrap().len(), 2);
        assert!(set
            .as_array()
            .unwrap()
            .iter()
            .all(|q| q["type"] == "General"));
        assert_ne!(set[0]["question"], set[1]["question"]);
    }
    let (_, again) = send(&app, generate_request(pool_id, options)).await;
    assert_eq!(first["sets"], again["sets"]);
    let (_, shifted) = send(
        &app,
        generate_request(
            pool_id,
            json!({
                "question_type": "custom", "custom_counts": {"General": 2}, "seed": 43,
            }),
        ),
    )
    .await;
    assert_eq!(first["sets"][1], shifted["sets"][0]);

    for (options, expected) in [
        (json!({"question_type": "custom"}), "requires custom_counts"),
        (
            json!({"question_type": "custom", "custom_counts": {}}),
            "at least one question",
        ),
        (
            json!({"question_type": "custom", "custom_counts": {"Quote": 1, "General": 4}}),
            "Quote (need 1, available 0)",
        ),
        (
            json!({"question_type": "standard", "custom_counts": {"General": 2}}),
            "only supported in Custom mode",
        ),
    ] {
        let (status, body) = send(&app, generate_request(pool_id, options)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains(expected), "{body}");
        assert!(body.get("sets").is_none());
    }
    for counts in [
        json!({"General": -1}),
        json!({"General": 0.5}),
        json!({"Unknown": 1}),
    ] {
        let (status, _) = send(
            &app,
            generate_request(
                pool_id,
                json!({
                    "question_type": "custom", "custom_counts": counts,
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
}

#[sqlx::test]
async fn custom_validation_reports_availability_and_feasibility_without_generating(pool: PgPool) {
    let app = app(&pool);
    let pool_id = create_pool(&pool, "public-pool", public_tier(&pool).await).await;
    let csv = "Question,Type,Reference,Answer\n\
        S1,Situation,John 1:1,A\nS2,Situation,John 1:2,A\nS3,Situation,John 1:3,A\n\
        G1,General,John 2:1,A\nG2,General,John 2:2,A\nG3,General,John 2:3,A\n";
    sqlx::query("UPDATE question_pools SET csv_text = $1, updated_at = NOW() WHERE id = $2")
        .bind(csv)
        .bind(pool_id)
        .execute(&pool)
        .await
        .unwrap();
    for (counts, end, valid, error) in [
        (json!({}), 2, false, "at least one question"),
        (
            json!({"Situation": 4}),
            2,
            false,
            "Situation (need 4, available 3)",
        ),
        (
            json!({"Situation": 3, "General": 1}),
            2,
            false,
            "limit of 2",
        ),
        (json!({"Situation": 2, "General": 2}), 2, true, ""),
        (json!({"Situation": 3}), 1, true, ""),
        (
            json!({"General": 1}),
            1,
            false,
            "General (need 1, available 0)",
        ),
    ] {
        let (status, body) = send(
            &app,
            json_request(
                Method::POST,
                "/api/generate/validate",
                None,
                json!({
                    "pool_id": pool_id, "question_type": "custom", "count": 0,
                    "custom_counts": counts,
                    "books": [{"name": "John", "start_chapter": 1, "end_chapter": end}],
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["valid"], valid);
        assert_eq!(body["available"]["Situation"], 3);
        assert_eq!(body["available"]["General"], if end == 1 { 0 } else { 3 });
        assert_eq!(body["available"]["Quote"], 0);
        assert_eq!(body["available"].as_object().unwrap().len(), 8);
        assert!(body.get("sets").is_none());
        if valid {
            assert!(body["error"].is_null());
        } else {
            assert!(body["error"].as_str().unwrap().contains(error), "{body}");
        }
    }
    let generated: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_logs WHERE action = 'questions.generated'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(generated, 0);

    let restricted = create_pool(&pool, "restricted", create_tier(&pool, "higher", 1).await).await;
    let (status, _) = send(
        &app,
        json_request(
            Method::POST,
            "/api/generate/validate",
            None,
            json!({
                "pool_id": restricted, "question_type": "custom", "count": 1,
                "custom_counts": {"General": 1},
                "books": [{"name": "John", "start_chapter": 1, "end_chapter": 2}],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn custom_generation_scales_the_chapter_cap_after_filtering(pool: PgPool) {
    let app = app(&pool);
    let pool_id = create_pool(&pool, "public-pool", public_tier(&pool).await).await;
    let csv = "Question,Type,Reference,Answer\n\
        S1,Situation,John 1:1,A\nS2,Situation,John 1:2,A\nS3,Situation,John 1:3,A\n\
        G1,General,John 2:1,A\nG2,General,John 2:2,A\nG3,General,John 2:3,A\n";
    sqlx::query("UPDATE question_pools SET csv_text = $1, updated_at = NOW() WHERE id = $2")
        .bind(csv)
        .bind(pool_id)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = send(
        &app,
        generate_request(
            pool_id,
            json!({
                "question_type": "custom",
                "custom_counts": {"Situation": 3, "General": 1},
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("limit of 2 questions per chapter"));
    assert!(body.get("sets").is_none());

    let (status, body) = send(
        &app,
        generate_request(
            pool_id,
            json!({
                "question_type": "custom",
                "custom_counts": {"Situation": 3},
                "books": [{"name": "John", "start_chapter": 1, "end_chapter": 1}],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sets"][0].as_array().unwrap().len(), 3);
    assert!(body["sets"][0].as_array().unwrap().iter().all(
        |q| q["type"] == "Situation" && q["reference"].as_str().unwrap().starts_with("John 1:")
    ));
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

async fn header_html(app: &Router, uri: &str, session: Option<&str>) -> String {
    let response = app.clone().oneshot(get(uri, session)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    let start = html.find("<header").expect("page has a header");
    let end = html.find("</header>").expect("header closes");
    html[start..end].to_string()
}

#[sqlx::test]
async fn every_page_renders_the_same_header_for_the_same_visitor(pool: PgPool) {
    let app = app(&pool);
    let public = public_tier(&pool).await;
    let admin = create_user(&pool, "the-admin", public, true, true).await;
    let reader = create_user(&pool, "the-reader", public, false, false).await;
    let admin_session = session(&pool, admin).await;
    let reader_session = session(&pool, reader).await;

    // Anonymous visitors can only reach / and /login.
    let anonymous = header_html(&app, "/", None).await;
    assert_eq!(header_html(&app, "/login", None).await, anonymous);
    assert!(anonymous.contains(r#"href="/login""#));
    assert!(!anonymous.contains(r#"id="logout-link""#));
    assert!(!anonymous.contains("user-menu"));
    // Signed-out visitors still get a light/dark control.
    assert!(anonymous.contains(r#"id="theme-button""#));

    let admin_header = header_html(&app, "/", Some(&admin_session)).await;
    for page in ["/login", "/account", "/admin"] {
        assert_eq!(
            header_html(&app, page, Some(&admin_session)).await,
            admin_header,
            "{page}"
        );
    }
    for expected in [
        "the-admin",
        r#"id="user-menu-button""#,
        r#"href="/account""#,
        r#"href="/admin""#,
        r#"id="theme-toggle""#,
        r#"id="logout-link""#,
    ] {
        assert!(admin_header.contains(expected), "missing {expected}");
    }

    // Users without admin or upload access do not see the Admin link, on any page.
    let reader_header = header_html(&app, "/", Some(&reader_session)).await;
    assert_eq!(
        header_html(&app, "/account", Some(&reader_session)).await,
        reader_header
    );
    assert!(reader_header.contains(r#"href="/account""#));
    assert!(!reader_header.contains(r#"href="/admin""#));
}

// ── Accounts: registration, email verification, password reset ──

use qsets_server::services::mail::CaptureMailer;
use std::sync::Arc;

fn mail_app(pool: &PgPool) -> (Router, Arc<CaptureMailer>) {
    let mailer = Arc::new(CaptureMailer::default());
    let state = AppState::new(pool.clone(), 2, false)
        .with_mailer(mailer.clone())
        .with_public_base_url("https://qsets.test/");
    (qsets_server::app(state, "static"), mailer)
}

/// Emails are sent from background tasks, so wait until `count` have arrived.
async fn wait_for_mail(
    mailer: &CaptureMailer,
    count: usize,
) -> Vec<qsets_server::services::mail::SentMail> {
    for _ in 0..100 {
        let sent = mailer.sent();
        if sent.len() >= count {
            return sent;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("expected {count} emails, got {}", mailer.sent().len());
}

fn token_in(mail: &qsets_server::services::mail::SentMail) -> String {
    let start = mail.body.find("token=").expect("mail has a link") + "token=".len();
    mail.body[start..]
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .collect()
}

async fn post(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    send(app, json_request(Method::POST, uri, None, body)).await
}

async fn register(app: &Router, username: &str, email: &str) -> (StatusCode, Value) {
    post(
        app,
        "/api/register",
        json!({"username": username, "email": email, "password": PASSWORD}),
    )
    .await
}

async fn login_as(app: &Router, identifier: &str, password: &str) -> (StatusCode, Value) {
    post(
        app,
        "/api/login",
        json!({"username": identifier, "password": password}),
    )
    .await
}

/// Registers and confirms an account, returning its mailer for further emails.
async fn confirmed_account(
    pool: &PgPool,
    username: &str,
    email: &str,
) -> (Router, Arc<CaptureMailer>) {
    let (app, mailer) = mail_app(pool);
    assert_eq!(register(&app, username, email).await.0, StatusCode::OK);
    let token = token_in(&wait_for_mail(&mailer, 1).await[0]);
    let (status, _) = post(&app, "/api/email/verify", json!({"token": token})).await;
    assert_eq!(status, StatusCode::OK);
    (app, mailer)
}

#[sqlx::test]
async fn registering_requires_confirming_the_email_before_signing_in(pool: PgPool) {
    let (app, mailer) = mail_app(&pool);

    let (status, _) = register(&app, "alice", "Alice@Example.com").await;
    assert_eq!(status, StatusCode::OK);
    let mail = wait_for_mail(&mailer, 1).await.remove(0);
    assert_eq!(mail.to, "Alice@Example.com");
    assert!(mail.body.contains("https://qsets.test/verify-email?token="));

    let (status, body) = login_as(&app, "alice", PASSWORD).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["needs_verification"], true);
    let (status, _) = login_as(&app, "alice@example.com", PASSWORD).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "unverified addresses cannot be used to sign in"
    );

    let token = token_in(&mail);
    let (status, _) = post(&app, "/api/email/verify", json!({"token": token})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = post(&app, "/api/email/verify", json!({"token": token})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a link works once");

    assert_eq!(login_as(&app, "alice", PASSWORD).await.0, StatusCode::OK);
    assert_eq!(
        login_as(&app, "ALICE@example.COM", PASSWORD).await.0,
        StatusCode::OK
    );
    assert_eq!(
        login_as(&app, "alice", "wrong password").await.0,
        StatusCode::UNAUTHORIZED
    );

    let tier: i32 = sqlx::query_scalar(
        "SELECT t.rank FROM users u JOIN tiers t ON t.id = u.tier_id WHERE username = 'alice'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(tier, 0, "self-registered accounts get public access only");
}

#[sqlx::test]
async fn registration_validates_input_and_rejects_taken_usernames(pool: PgPool) {
    let (app, _mailer) = mail_app(&pool);

    for (username, email) in [
        ("ab", "a@b.co"),
        ("a@b.co", "a@b.co"),
        ("alice", "not-an-email"),
    ] {
        assert_eq!(
            register(&app, username, email).await.0,
            StatusCode::BAD_REQUEST,
            "{username} {email}"
        );
    }
    let (status, _) = post(
        &app,
        "/api/register",
        json!({"username": "alice", "email": "a@b.co", "password": "short"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    assert_eq!(register(&app, "Alice", "a@b.co").await.0, StatusCode::OK);
    assert_eq!(
        register(&app, "alice", "other@b.co").await.0,
        StatusCode::CONFLICT
    );
}

#[sqlx::test]
async fn an_address_with_an_account_gets_a_notice_not_a_second_account(pool: PgPool) {
    let (app, mailer) = confirmed_account(&pool, "alice", "alice@example.com").await;

    let first = register(&app, "mallory", "alice@example.com").await;
    assert_eq!(first.0, StatusCode::OK);
    let sent = wait_for_mail(&mailer, 2).await;
    assert!(sent[1].subject.contains("already has an account"));
    assert!(!sent[1].body.contains("token="));

    let accounts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE username = 'mallory'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(accounts, 0);
    // The caller sees the same answer as for a brand-new address.
    let fresh = register(&app, "bob", "bob@example.com").await;
    assert_eq!(first, fresh);
}

#[sqlx::test]
async fn registration_can_be_closed(pool: PgPool) {
    let state = AppState::new(pool.clone(), 2, false).with_registration_open(false);
    let app = qsets_server::app(state, "static");
    assert_eq!(
        register(&app, "alice", "a@b.co").await.0,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test]
async fn forgotten_passwords_are_reset_through_an_emailed_link(pool: PgPool) {
    let (app, mailer) = confirmed_account(&pool, "alice", "alice@example.com").await;
    let user_id: i32 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'alice'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let old_session = session(&pool, user_id).await;

    let (status, requested) = post(
        &app,
        "/api/password-reset/request",
        json!({"email": "ALICE@example.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mail = wait_for_mail(&mailer, 2).await.remove(1);
    assert!(mail
        .body
        .contains("https://qsets.test/reset-password?token="));
    let token = token_in(&mail);

    let (status, _) = post(
        &app,
        "/api/password-reset/confirm",
        json!({"token": token, "new_password": "short"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post(
        &app,
        "/api/password-reset/confirm",
        json!({"token": "0".repeat(64), "new_password": "a brand new password"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = post(
        &app,
        "/api/password-reset/confirm",
        json!({"token": token, "new_password": "a brand new password"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = post(
        &app,
        "/api/password-reset/confirm",
        json!({"token": token, "new_password": "another new password"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a reset link works once");

    assert_eq!(
        login_as(&app, "alice", PASSWORD).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        login_as(&app, "alice@example.com", "a brand new password")
            .await
            .0,
        StatusCode::OK
    );
    let (status, _) = send(&app, get("/api/me", Some(&old_session))).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "resetting signs out existing sessions"
    );

    // An address nobody owns gets the same answer and no email.
    let (status, unknown) = post(
        &app,
        "/api/password-reset/request",
        json!({"email": "nobody@example.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(unknown, requested);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(mailer.sent().len(), 2);
}

#[sqlx::test]
async fn unconfirmed_addresses_cannot_be_used_to_reset_passwords(pool: PgPool) {
    let (app, mailer) = mail_app(&pool);
    register(&app, "alice", "alice@example.com").await;
    wait_for_mail(&mailer, 1).await;

    let (status, _) = post(
        &app,
        "/api/password-reset/request",
        json!({"email": "alice@example.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(mailer.sent().len(), 1);
}

#[sqlx::test]
async fn resending_confirmation_mails_a_fresh_link_and_retires_the_old_one(pool: PgPool) {
    let (app, mailer) = mail_app(&pool);
    register(&app, "alice", "alice@example.com").await;
    let first = token_in(&wait_for_mail(&mailer, 1).await[0]);

    let (status, _) = post(
        &app,
        "/api/email/resend",
        json!({"email": "alice@example.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let second = token_in(&wait_for_mail(&mailer, 2).await[1]);
    assert_ne!(first, second);

    let (status, _) = post(&app, "/api/email/verify", json!({"token": first})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post(&app, "/api/email/verify", json!({"token": second})).await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test]
async fn email_requests_are_limited_per_address(pool: PgPool) {
    let (app, _mailer) = mail_app(&pool);
    for _ in 0..3 {
        let (status, _) = post(
            &app,
            "/api/password-reset/request",
            json!({"email": "a@b.co"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _) = post(
        &app,
        "/api/password-reset/request",
        json!({"email": "A@b.co"}),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[sqlx::test]
async fn changing_the_email_needs_the_password_and_confirmation_of_the_new_address(pool: PgPool) {
    let (app, mailer) = confirmed_account(&pool, "alice", "old@example.com").await;
    let user_id: i32 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'alice'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let alice = session(&pool, user_id).await;
    let change = |password: &str| {
        json_request(
            Method::POST,
            "/api/me/email",
            Some(&alice),
            json!({"email": "new@example.com", "current_password": password}),
        )
    };

    let (status, _) = send(&app, change("wrong password")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&app, change(PASSWORD)).await;
    assert_eq!(status, StatusCode::OK);
    let mail = wait_for_mail(&mailer, 2).await.remove(1);
    assert_eq!(mail.to, "new@example.com");

    let (_, me) = send(&app, get("/api/me", Some(&alice))).await;
    assert_eq!(
        me["email"], "old@example.com",
        "nothing changes until the link is opened"
    );

    let (status, _) = post(&app, "/api/email/verify", json!({"token": token_in(&mail)})).await;
    assert_eq!(status, StatusCode::OK);
    let (_, me) = send(&app, get("/api/me", Some(&alice))).await;
    assert_eq!(me["email"], "new@example.com");
    assert_eq!(
        login_as(&app, "new@example.com", PASSWORD).await.0,
        StatusCode::OK
    );
    assert_eq!(
        login_as(&app, "old@example.com", PASSWORD).await.0,
        StatusCode::UNAUTHORIZED
    );

    let (status, _) = send(&app, get("/api/me", None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let anonymous = json_request(
        Method::POST,
        "/api/me/email",
        None,
        json!({"email": "x@y.co", "current_password": PASSWORD}),
    );
    assert_eq!(send(&app, anonymous).await.0, StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn admin_set_emails_work_for_login_and_must_be_unique(pool: PgPool) {
    let (app, _mailer) = mail_app(&pool);
    let tier = public_tier(&pool).await;
    let admin = create_user(&pool, "root", tier, true, true).await;
    let admin = session(&pool, admin).await;
    let create = |username: &str, email: &str| {
        json_request(
            Method::POST,
            "/api/admin/users",
            Some(&admin),
            json!({"username": username, "password": PASSWORD, "tier_id": tier,
                   "is_admin": false, "email": email}),
        )
    };

    let (status, body) = send(&app, create("carol", "carol@example.com")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["email_verified"], true);
    assert_eq!(
        login_as(&app, "carol@example.com", PASSWORD).await.0,
        StatusCode::OK
    );

    let (status, _) = send(&app, create("dave", "CAROL@example.com")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = send(&app, create("a@b.co", "")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn sweep_removes_stale_unconfirmed_signups_but_keeps_everyone_else(pool: PgPool) {
    let (app, mailer) = mail_app(&pool);
    register(&app, "stale", "stale@example.com").await;
    register(&app, "fresh", "fresh@example.com").await;
    wait_for_mail(&mailer, 2).await;
    let tier = public_tier(&pool).await;
    create_user(&pool, "legacy", tier, false, false).await;
    sqlx::query("UPDATE users SET created_at = NOW() - INTERVAL '4 days'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET created_at = NOW() WHERE username = 'fresh'")
        .execute(&pool)
        .await
        .unwrap();

    qsets_server::services::accounts::sweep(&pool)
        .await
        .unwrap();

    let names: Vec<String> = sqlx::query_scalar("SELECT username FROM users ORDER BY username")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(names, ["fresh", "legacy"]);
}

#[sqlx::test]
async fn account_pages_render(pool: PgPool) {
    let (app, mailer) = confirmed_account(&pool, "alice", "alice@example.com").await;
    let _ = mailer;
    let user_id: i32 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'alice'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let alice = session(&pool, user_id).await;

    for uri in [
        "/register",
        "/forgot-password",
        "/reset-password",
        "/verify-email",
        "/login",
    ] {
        let response = app.clone().oneshot(get(uri, None)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
    }
    let response = app
        .clone()
        .oneshot(get("/account", Some(&alice)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&bytes).contains("alice@example.com"));
}
