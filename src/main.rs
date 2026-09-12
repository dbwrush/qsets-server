mod models;
mod routes;
mod services;
mod state;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::{routing::get_service, Router};
use sqlx::postgres::PgPoolOptions;
use tower_http::services::ServeDir;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    routes::{api, pages},
    services::auth,
    state::AppState,
};

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
    if app_env != "development" && admin_password == "change-me" {
        panic!("ADMIN_PASSWORD must be changed outside the development environment");
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

    let state = AppState {
        pool,
        parsed_pools: Arc::default(),
        generation_slots: Arc::new(tokio::sync::Semaphore::new(generation_concurrency)),
    };

    let app = Router::new()
        .merge(api::router(state.clone()))
        .merge(pages::router(state.clone()))
        .nest_service("/static", get_service(ServeDir::new("static")));

    let addr: SocketAddr = bind_addr.parse().expect("invalid BIND_ADDR");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind failed");

    tracing::info!("listening on {}", addr);
    axum::serve(listener, app).await.expect("server failed");
}
