use askama::Template;
use axum::{
    extract::State,
    response::{Html, IntoResponse},
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
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/login", get(login))
        .route("/admin", get(admin_page))
        .with_state(state)
}

async fn index(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    let (jar, csrf) = security::ensure_csrf_cookie(jar);

    let (is_authenticated, username) =
        if let Some(user_id) = auth::get_user_id_from_jar(&state.pool, &jar).await {
            let user = sqlx::query_as::<_, (String,)>("SELECT username FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_optional(&state.pool)
                .await
                .unwrap_or(None);
            match user {
                Some((uname,)) => (true, uname),
                None => (false, String::new()),
            }
        } else {
            (false, String::new())
        };

    let page = IndexTemplate {
        csrf_token: &csrf,
        is_authenticated,
        username: &username,
    };
    (
        jar,
        Html(
            page.render()
                .unwrap_or_else(|_| "Template error".to_string()),
        ),
    )
}

async fn login(jar: CookieJar) -> impl IntoResponse {
    let (jar, csrf) = security::ensure_csrf_cookie(jar);
    let page = LoginTemplate { csrf_token: &csrf };
    (
        jar,
        Html(
            page.render()
                .unwrap_or_else(|_| "Template error".to_string()),
        ),
    )
}

async fn admin_page(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    let Some(user_id) = auth::get_user_id_from_jar(&state.pool, &jar).await else {
        return (jar, Html("Unauthorized".to_string())).into_response();
    };

    let user_row =
        sqlx::query_as::<_, (String, bool)>("SELECT username, is_admin FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);

    let Some((username, is_admin)) = user_row else {
        return (jar, Html("Forbidden".to_string())).into_response();
    };

    if !is_admin {
        return (jar, Html("Forbidden".to_string())).into_response();
    }

    let (jar, csrf) = security::ensure_csrf_cookie(jar);
    let page = AdminTemplate {
        csrf_token: &csrf,
        username: &username,
    };
    (
        jar,
        Html(
            page.render()
                .unwrap_or_else(|_| "Template error".to_string()),
        ),
    )
        .into_response()
}
