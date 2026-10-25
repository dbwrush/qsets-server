//! A pool's per-type question counts, stored beside its CSV so the generator can see which
//! question types a pool offers without parsing it.

use serde_json::{json, Value};
use sqlx::PgPool;

use super::generator::{available_type_counts, parse_csv, QuestionRecord};

pub fn type_counts(questions: &[QuestionRecord]) -> Value {
    json!(available_type_counts(questions))
}

pub fn type_counts_for_csv(csv: &str) -> Value {
    type_counts(&parse_csv(csv))
}

/// Fills in the counts for pools saved before they were tracked.
pub async fn backfill(pool: &PgPool) -> Result<usize, sqlx::Error> {
    let pending: Vec<(i32, String)> =
        sqlx::query_as("SELECT id, csv_text FROM question_pools WHERE type_counts = '{}'::jsonb")
            .fetch_all(pool)
            .await?;
    for (id, csv) in &pending {
        sqlx::query("UPDATE question_pools SET type_counts = $1 WHERE id = $2")
            .bind(type_counts_for_csv(csv))
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(pending.len())
}
