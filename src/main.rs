use std::collections::HashSet;
use std::net::SocketAddr;
use std::time::Duration;

use qsets_server::{
    services::{audit, auth},
    state::AppState,
};
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const MAINTENANCE_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);
const DEFAULT_AUDIT_RETENTION_DAYS: u32 = 365;

fn load_env_file(path: &str, shell_keys: &HashSet<String>, allow_override_non_shell: bool) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key_raw, value_raw)) = line.split_once('=') else {
            continue;
        };

        let key = key_raw.trim();
        if key.is_empty() || shell_keys.contains(key) {
            continue;
        }

        let value = value_raw.trim().trim_matches('"').trim_matches('\'');

        if allow_override_non_shell || std::env::var(key).is_err() {
            std::env::set_var(key, value);
        }
    }
}

fn load_environment_profiles() {
    let shell_keys: HashSet<String> = std::env::vars().map(|(k, _)| k).collect();

    load_env_file(".env", &shell_keys, false);

    let app_env = std::env::var("APP_ENV").unwrap_or_else(|_| "development".to_string());
    let env_path = format!(".env.{app_env}");
    load_env_file(&env_path, &shell_keys, true);
}

#[tokio::main]
async fn main() {
    load_environment_profiles();
    let app_env = std::env::var("APP_ENV").unwrap_or_else(|_| "development".to_string());

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "qsets_server=debug,tower_http=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL is required");
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    let admin_username = std::env::var("ADMIN_USERNAME").unwrap_or_else(|_| "admin".to_string());
    let admin_password = std::env::var("ADMIN_PASSWORD")
        .expect("ADMIN_PASSWORD is required; set a strong value before starting the server");
    if app_env != "development" {
        if admin_password == "change-me" {
            panic!("ADMIN_PASSWORD must be changed outside the development environment");
        }
        if let Err(message) = auth::validate_password(&admin_password) {
            panic!("ADMIN_PASSWORD is too weak: {message}");
        }
    }
    let trust_proxy = std::env::var("TRUST_PROXY")
        .map(|v| v == "true")
        .unwrap_or(false);
    let static_dir = std::env::var("STATIC_DIR").unwrap_or_else(|_| "static".to_string());
    if !std::path::Path::new(&static_dir).is_dir() {
        tracing::warn!(
            static_dir,
            "static asset directory not found; set STATIC_DIR or start from the repository root"
        );
    }
    let generation_concurrency = std::env::var("GENERATION_CONCURRENCY")
        .map(|value| {
            value
                .parse::<usize>()
                .expect("GENERATION_CONCURRENCY must be a positive integer")
        })
        .unwrap_or_else(|_| {
            std::thread::available_parallelism()
                .map(|parallelism| parallelism.get())
                .unwrap_or(2)
        })
        .max(1);
    // 0 keeps audit entries forever.
    let audit_retention_days = std::env::var("AUDIT_RETENTION_DAYS")
        .map(|value| {
            value
                .parse::<u32>()
                .expect("AUDIT_RETENTION_DAYS must be a non-negative whole number of days")
        })
        .unwrap_or(DEFAULT_AUDIT_RETENTION_DAYS)
        .min(audit::MAX_RETENTION_DAYS);

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .expect("failed to connect to database");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("failed to run migrations");

    auth::create_default_admin(&pool, &admin_username, &admin_password)
        .await
        .expect("failed to create default admin");

    auth::prepare_dummy_hash();

    let sweep_pool = pool.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(MAINTENANCE_SWEEP_INTERVAL);
        loop {
            interval.tick().await;
            match auth::delete_expired_sessions(&sweep_pool).await {
                Ok(0) => {}
                Ok(removed) => tracing::debug!(removed, "deleted expired sessions"),
                Err(error) => tracing::warn!(error = %error, "failed to delete expired sessions"),
            }
            if audit_retention_days > 0 {
                match audit::delete_older_than(&sweep_pool, audit_retention_days).await {
                    Ok(0) => {}
                    Ok(removed) => tracing::info!(removed, "deleted expired audit entries"),
                    Err(error) => {
                        tracing::warn!(error = %error, "failed to delete expired audit entries")
                    }
                }
            }
        }
    });

    let state = AppState::new(pool, generation_concurrency, trust_proxy);
    let app = qsets_server::app(state, &static_dir);

    let addr: SocketAddr = bind_addr.parse().expect("invalid BIND_ADDR");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind failed");

    tracing::info!("listening on {}", addr);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("server failed");
}
