use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct User {
    pub id: i32,
    pub username: String,
    pub password_hash: String,
    pub is_admin: bool,
    pub tier_id: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Tier {
    pub id: i32,
    pub name: String,
    pub rank: i32,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct QuestionPool {
    pub id: i32,
    pub name: String,
    pub tier_id: i32,
    pub csv_text: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
