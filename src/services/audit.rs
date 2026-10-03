use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;

use crate::services::security::ClientMeta;

/// Records an action. Failures are logged rather than returned: an audit write must never undo
/// or block the change it describes.
pub async fn record(
    pool: &PgPool,
    client: &ClientMeta,
    actor_user_id: Option<i32>,
    action: &str,
    target_type: &str,
    target_id: impl ToString,
    metadata: Value,
) {
    if let Err(error) = sqlx::query(
        "INSERT INTO audit_logs (actor_user_id, action, target_type, target_id, metadata, ip_address, user_agent)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(actor_user_id)
    .bind(action)
    .bind(target_type)
    .bind(target_id.to_string())
    .bind(metadata)
    .bind(&client.ip)
    .bind(&client.user_agent)
    .execute(pool)
    .await
    {
        tracing::error!(action, error = %error, "failed to write audit event");
    }
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AuditEntry {
    pub id: i64,
    pub created_at: DateTime<Utc>,
    pub actor_user_id: Option<i32>,
    pub actor_username: Option<String>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub metadata: Value,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

/// Newest entries first. `before` is the id of the last entry already shown, for paging, and
/// `action_prefix` narrows to e.g. `login.` or `pool.`.
pub async fn list(
    pool: &PgPool,
    before: Option<i64>,
    action_prefix: Option<&str>,
    limit: i64,
) -> Result<Vec<AuditEntry>, sqlx::Error> {
    sqlx::query_as::<_, AuditEntry>(
        "SELECT a.id, a.created_at, a.actor_user_id, u.username AS actor_username, a.action,
                a.target_type, a.target_id, a.metadata, a.ip_address, a.user_agent
         FROM audit_logs a
         LEFT JOIN users u ON u.id = a.actor_user_id
         WHERE ($1::bigint IS NULL OR a.id < $1)
           AND ($2::text IS NULL OR starts_with(a.action, $2))
         ORDER BY a.id DESC
         LIMIT $3",
    )
    .bind(before)
    .bind(action_prefix)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Longest retention accepted; anything larger would push the cutoff past timestamp range.
pub const MAX_RETENTION_DAYS: u32 = 36_500;

/// Deletes entries older than `days` days (capped at [`MAX_RETENTION_DAYS`]).
pub async fn delete_older_than(pool: &PgPool, days: u32) -> Result<u64, sqlx::Error> {
    sqlx::query("DELETE FROM audit_logs WHERE created_at < NOW() - make_interval(days => $1)")
        .bind(days.min(MAX_RETENTION_DAYS) as i32)
        .execute(pool)
        .await
        .map(|result| result.rows_affected())
}
