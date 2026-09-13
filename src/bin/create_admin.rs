//! Standalone CLI for creating (or promoting) an admin account.
//! Invoked via scripts/create_admin_user.sh, which supplies credentials as env vars
//! so passwords never appear in shell history or process argument lists.

use qsets_server::services::auth;
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() {
    let username = std::env::var("ADMIN_NEW_USERNAME")
        .expect("ADMIN_NEW_USERNAME is required; run this via scripts/create_admin_user.sh");
    let password = std::env::var("ADMIN_NEW_PASSWORD")
        .expect("ADMIN_NEW_PASSWORD is required; run this via scripts/create_admin_user.sh");
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL is required");

    if username.trim().is_empty() {
        eprintln!("Username must not be empty");
        std::process::exit(1);
    }
    if password.len() < 8 {
        eprintln!("Password must be at least 8 characters long");
        std::process::exit(1);
    }

    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("failed to connect to database");

    let tier_id: Option<i32> =
        sqlx::query_scalar("SELECT id FROM tiers ORDER BY rank DESC LIMIT 1")
            .fetch_optional(&pool)
            .await
            .expect("failed to query tiers");

    // Admins need a tier for the NOT NULL column even though tier checks bypass them.
    let Some(tier_id) = tier_id else {
        eprintln!(
            "No tiers exist yet. Start the server once (it runs migrations) and create a tier first."
        );
        std::process::exit(1);
    };

    let hash = auth::hash_password(&password).expect("failed to hash password");

    sqlx::query(
        "INSERT INTO users (username, password_hash, is_admin, can_upload_pools, tier_id)
         VALUES ($1, $2, true, true, $3)
         ON CONFLICT (username) DO UPDATE
             SET password_hash = EXCLUDED.password_hash,
                 is_admin = true,
                 can_upload_pools = true",
    )
    .bind(&username)
    .bind(hash)
    .bind(tier_id)
    .execute(&pool)
    .await
    .expect("failed to create/update admin user");

    println!("Admin user '{username}' is ready.");
}
