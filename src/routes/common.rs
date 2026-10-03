use std::borrow::Cow;

use axum::{
    async_trait,
    extract::{FromRequestParts, Request},
    http::{request::Parts, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::extract::cookie::CookieJar;
use serde_json::{json, Value};

use crate::{
    services::{authz::Actor, security},
    state::AppState,
};

/// An API failure, rendered as `{"error": message}` plus any `details` fields.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: Cow<'static, str>,
    details: Option<Value>,
}

pub type ApiResult<T = Response> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            status,
            message: message.into(),
            details: None,
        }
    }

    pub fn bad_request(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Not authenticated")
    }

    pub fn forbidden(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    pub fn not_found(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    pub fn conflict(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    pub fn internal(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    /// Extra top-level fields merged into the response body next to `error`.
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = json!({ "error": self.message });
        if let (Some(Value::Object(details)), Value::Object(map)) = (self.details, &mut body) {
            map.extend(details);
        }
        (self.status, Json(body)).into_response()
    }
}

/// Maps a database error to a 500 after logging it. Use for failures the caller cannot fix.
pub fn db_error(message: &'static str) -> impl FnOnce(sqlx::Error) -> ApiError {
    move |error| {
        tracing::error!(error = %error, "{message}");
        ApiError::internal(message)
    }
}

pub fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.is_unique_violation())
}

/// A write or delete blocked by a foreign key. `ON DELETE RESTRICT` reports `restrict_violation`
/// (23001) rather than the usual `foreign_key_violation` (23503), so both count.
pub fn is_foreign_key_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db)
        if db.is_foreign_key_violation() || db.code().as_deref() == Some("23001"))
}

/// The signed-in user's id; rejects anonymous callers.
pub fn require_user(actor: &Actor) -> ApiResult<i32> {
    actor.user_id.ok_or_else(ApiError::unauthorized)
}

/// Extracts the id of a signed-in admin, rejecting everyone else with 401 or 403.
pub struct Admin(pub i32);

#[async_trait]
impl FromRequestParts<AppState> for Admin {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Ok(actor) = Actor::from_request_parts(parts, state).await;
        let user_id = require_user(&actor)?;
        if !actor.is_admin {
            return Err(ApiError::forbidden("Admin required"));
        }
        Ok(Self(user_id))
    }
}

/// Rejects state-changing API requests whose `x-csrf-token` header does not match the CSRF
/// cookie. Runs before the handler, so no body is parsed for a forged request.
pub async fn require_csrf(request: Request, next: Next) -> Response {
    let safe_method = matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    let jar = CookieJar::from_headers(request.headers());
    if !safe_method && !security::verify_csrf(&jar, request.headers()) {
        return ApiError::forbidden("CSRF validation failed").into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn error_details_are_merged_into_the_body() {
        let response = ApiError::bad_request("bad csv")
            .with_details(json!({"validation": {"valid_count": 0}}))
            .into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"], "bad csv");
        assert_eq!(body["validation"]["valid_count"], 0);
    }
}
