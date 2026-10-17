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

/// What the shared header (`_header.html`) shows; every page renders the same one.
struct Header {
    is_authenticated: bool,
    username: String,
    /// First letter of the username, shown in the menu button's avatar.
    initial: String,
    can_access_admin: bool,
    can_manage_pools: bool,
}

impl Header {
    fn new(user: Option<&SignedInUser>) -> Self {
        Self {
            is_authenticated: user.is_some(),
            username: user.map(|u| u.username.clone()).unwrap_or_default(),
            initial: user
                .and_then(|u| u.username.chars().next())
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_default(),
            can_access_admin: user.is_some_and(|u| u.is_admin),
            can_manage_pools: user.is_some_and(|u| u.is_admin || u.can_upload_pools || u.has_pools),
        }
    }
}

struct SignedInUser {
    username: String,
    email: Option<String>,
    email_verified: bool,
    is_admin: bool,
    can_upload_pools: bool,
    has_pools: bool,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate<'a> {
    csrf_token: &'a str,
    header: Header,
}

#[derive(Template)]
#[template(path = "study.html")]
struct StudyTemplate<'a> {
    csrf_token: &'a str,
    header: Header,
}

#[derive(Template)]
#[template(path = "pool_editor.html")]
struct PoolEditorTemplate<'a> {
    csrf_token: &'a str,
    header: Header,
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate<'a> {
    csrf_token: &'a str,
    header: Header,
}

#[derive(Template)]
#[template(path = "admin.html")]
struct AdminTemplate<'a> {
    csrf_token: &'a str,
    header: Header,
    is_admin: bool,
}

#[derive(Template)]
#[template(path = "account.html")]
struct AccountTemplate<'a> {
    csrf_token: &'a str,
    header: Header,
    email: Option<String>,
    email_verified: bool,
}

#[derive(Template)]
#[template(path = "register.html")]
struct RegisterTemplate<'a> {
    csrf_token: &'a str,
    header: Header,
    registration_open: bool,
}

#[derive(Template)]
#[template(path = "forgot_password.html")]
struct ForgotPasswordTemplate<'a> {
    csrf_token: &'a str,
    header: Header,
}

#[derive(Template)]
#[template(path = "reset_password.html")]
struct ResetPasswordTemplate<'a> {
    csrf_token: &'a str,
    header: Header,
}

#[derive(Template)]
#[template(path = "verify_email.html")]
struct VerifyEmailTemplate<'a> {
    csrf_token: &'a str,
    header: Header,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/study", get(study_page))
        .route("/login", get(login))
        .route("/register", get(register_page))
        .route("/forgot-password", get(forgot_password_page))
        .route("/reset-password", get(reset_password_page))
        .route("/verify-email", get(verify_email_page))
        .route("/account", get(account_page))
        .route("/admin", get(admin_page))
        .route("/pools", get(admin_page))
        .route("/pools/new", get(pool_editor_page))
        .route("/pools/:id/edit", get(pool_editor_page))
        .with_state(state)
}

async fn study_page(State(state): State<AppState>, jar: CookieJar) -> Response {
    let (jar, csrf) = security::ensure_csrf_cookie(jar);
    let user = signed_in_user(&state, &jar).await;
    render(
        jar,
        StudyTemplate {
            csrf_token: &csrf,
            header: Header::new(user.as_ref()),
        },
    )
}

async fn index(State(state): State<AppState>, jar: CookieJar) -> Response {
    let (jar, csrf) = security::ensure_csrf_cookie(jar);
    let user = signed_in_user(&state, &jar).await;
    render(
        jar,
        IndexTemplate {
            csrf_token: &csrf,
            header: Header::new(user.as_ref()),
        },
    )
}

async fn login(State(state): State<AppState>, jar: CookieJar) -> Response {
    let (jar, csrf) = security::ensure_csrf_cookie(jar);
    let user = signed_in_user(&state, &jar).await;
    render(
        jar,
        LoginTemplate {
            csrf_token: &csrf,
            header: Header::new(user.as_ref()),
        },
    )
}

async fn register_page(State(state): State<AppState>, jar: CookieJar) -> Response {
    let (jar, csrf) = security::ensure_csrf_cookie(jar);
    let user = signed_in_user(&state, &jar).await;
    render(
        jar,
        RegisterTemplate {
            csrf_token: &csrf,
            header: Header::new(user.as_ref()),
            registration_open: state.registration_open,
        },
    )
}

async fn forgot_password_page(State(state): State<AppState>, jar: CookieJar) -> Response {
    let (jar, csrf) = security::ensure_csrf_cookie(jar);
    let user = signed_in_user(&state, &jar).await;
    render(
        jar,
        ForgotPasswordTemplate {
            csrf_token: &csrf,
            header: Header::new(user.as_ref()),
        },
    )
}

async fn reset_password_page(State(state): State<AppState>, jar: CookieJar) -> Response {
    let (jar, csrf) = security::ensure_csrf_cookie(jar);
    let user = signed_in_user(&state, &jar).await;
    render(
        jar,
        ResetPasswordTemplate {
            csrf_token: &csrf,
            header: Header::new(user.as_ref()),
        },
    )
}

async fn verify_email_page(State(state): State<AppState>, jar: CookieJar) -> Response {
    let (jar, csrf) = security::ensure_csrf_cookie(jar);
    let user = signed_in_user(&state, &jar).await;
    render(
        jar,
        VerifyEmailTemplate {
            csrf_token: &csrf,
            header: Header::new(user.as_ref()),
        },
    )
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

/// The signed-in user behind the session cookie, or `None` for anonymous visitors.
async fn signed_in_user(state: &AppState, jar: &CookieJar) -> Option<SignedInUser> {
    let user_id = auth::get_user_id_from_jar(&state.pool, jar).await?;
    sqlx::query_as::<_, (String, Option<String>, bool, bool, bool, bool)>(
        "SELECT username, email, email_verified_at IS NOT NULL, is_admin, can_upload_pools,
                EXISTS(SELECT 1 FROM question_pools WHERE created_by = users.id)
         FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None)
    .map(
        |(username, email, email_verified, is_admin, can_upload_pools, has_pools)| SignedInUser {
            username,
            email,
            email_verified,
            is_admin,
            can_upload_pools,
            has_pools,
        },
    )
}

async fn account_page(State(state): State<AppState>, jar: CookieJar) -> Response {
    let Some(user) = signed_in_user(&state, &jar).await else {
        return Redirect::to("/login").into_response();
    };
    let (jar, csrf) = security::ensure_csrf_cookie(jar);
    render(
        jar,
        AccountTemplate {
            csrf_token: &csrf,
            header: Header::new(Some(&user)),
            email: user.email,
            email_verified: user.email_verified,
        },
    )
}

async fn admin_page(State(state): State<AppState>, jar: CookieJar) -> Response {
    let Some(user) = signed_in_user(&state, &jar).await else {
        return Redirect::to("/login").into_response();
    };
    if !user.is_admin && !user.can_upload_pools && !user.has_pools {
        return forbidden_page();
    }

    let (jar, csrf) = security::ensure_csrf_cookie(jar);
    render(
        jar,
        AdminTemplate {
            csrf_token: &csrf,
            header: Header::new(Some(&user)),
            is_admin: user.is_admin,
        },
    )
}

async fn pool_editor_page(State(state): State<AppState>, jar: CookieJar) -> Response {
    let Some(user) = signed_in_user(&state, &jar).await else {
        return Redirect::to("/login").into_response();
    };
    let (jar, csrf) = security::ensure_csrf_cookie(jar);
    render(
        jar,
        PoolEditorTemplate {
            csrf_token: &csrf,
            header: Header::new(Some(&user)),
        },
    )
}
