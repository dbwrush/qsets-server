use serde_json::Value;
use sqlx::PgPool;

pub struct AuditEvent<'a> {
    pub actor_user_id: Option<i32>,
    pub action: &'a str,
    pub target_type: &'a str,
    pub target_id: Option<String>,
    pub metadata: Value,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

pub async fn log_event(pool: &PgPool, event: AuditEvent<'_>) {
    if let Err(error) = sqlx::query(
        "INSERT INTO audit_logs (actor_user_id, action, target_type, target_id, metadata, ip_address, user_agent)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(event.actor_user_id)
    .bind(event.action)
    .bind(event.target_type)
    .bind(event.target_id)
    .bind(event.metadata)
    .bind(event.ip_address)
    .bind(event.user_agent)
    .execute(pool)
    .await
    {
        tracing::error!(action = event.action, error = %error, "failed to write audit event");
    }
}
