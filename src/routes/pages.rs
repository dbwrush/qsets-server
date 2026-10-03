use askama::Template;
use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use axum_extra::extract::cookie::CookieJar;

use crate::{
    services::{auth, security},
    state::AppState,
};

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate<'a> {
    csrf_token: &'a str,
    is_authenticated: bool,
    username: &'a str,
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate<'a> {
    csrf_token: &'a str,
}

#[derive(Template)]
#[template(path = "admin.html")]
struct AdminTemplate<'a> {
    csrf_token: &'a str,
    username: &'a str,
    is_admin: bool,
}

#[derive(Template)]
#[template(path = "account.html")]
struct AccountTemplate<'a> {
    csrf_token: &'a str,
    username: &'a str,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/login", get(login))
        .route("/account", get(account_page))
        .route("/admin", get(admin_page))
        .with_state(state)
}

async fn index(State(state): State<AppState>, jar: CookieJar) -> Response {
    let (jar, csrf) = security::ensure_csrf_cookie(jar);
    let user = signed_in_user(&state, &jar).await;
    let username = user
        .as_ref()
        .map(|(name, _, _)| name.as_str())
        .unwrap_or("");

    render(
        jar,
        IndexTemplate {
            csrf_token: &csrf,
            is_authenticated: user.is_some(),
            username,
        },
    )
}

async fn login(jar: CookieJar) -> Response {
    let (jar, csrf) = security::ensure_csrf_cookie(jar);
    render(jar, LoginTemplate { csrf_token: &csrf })
}

fn render(jar: CookieJar, page: impl Template) -> Response {
    match page.render() {
        Ok(html) => (jar, Html(html)).into_response(),
        Err(error) => {
            tracing::error!(error = %error, "failed to render template");
            (StatusCode::INTERNAL_SERVER_ERROR, "Template error").into_response()
        }
    }
}

fn forbidden_page() -> Response {
    (StatusCode::FORBIDDEN, Html("Forbidden")).into_response()
}

/// Username and permissions of the signed-in user, or `None` for anonymous visitors.
async fn signed_in_user(state: &AppState, jar: &CookieJar) -> Option<(String, bool, bool)> {
    let user_id = auth::get_user_id_from_jar(&state.pool, jar).await?;
    sqlx::query_as::<_, (String, bool, bool)>(
        "SELECT username, is_admin, can_upload_pools FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None)
}

async fn account_page(State(state): State<AppState>, jar: CookieJar) -> Response {
    let Some((username, _, _)) = signed_in_user(&state, &jar).await else {
        return Redirect::to("/login").into_response();
    };
    let (jar, csrf) = security::ensure_csrf_cookie(jar);
    render(
        jar,
        AccountTemplate {
            csrf_token: &csrf,
            username: &username,
        },
    )
}

async fn admin_page(State(state): State<AppState>, jar: CookieJar) -> Response {
    let Some((username, is_admin, can_upload_pools)) = signed_in_user(&state, &jar).await else {
        return Redirect::to("/login").into_response();
    };
    if !is_admin && !can_upload_pools {
        return forbidden_page();
    }

    let (jar, csrf) = security::ensure_csrf_cookie(jar);
    render(
        jar,
        AdminTemplate {
            csrf_token: &csrf,
            username: &username,
            is_admin,
        },
    )
}
