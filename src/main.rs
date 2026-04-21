use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use anyhow::Context;
use axum::extract::State;
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post, put};
use axum::Json;
use axum::Router;
use chrono::Utc;
use reqwest::Client;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::info;

mod admin;
mod auth;
mod config;
mod gemini;
mod proxy;
mod state;
mod usage;

use config::{load_config, normalize_usage_log_dir};
use state::AppState;
use usage::load_call_records_from_disk;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "llm_router=info,tower_http=info".to_string()),
        )
        .init();

    let cfg_path = std::env::var("ROUTER_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config/router.json"));

    let cfg = load_config(&cfg_path).await?;
    let upstream_timeout_secs = std::env::var("ROUTER_UPSTREAM_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(45);

    let max_call_records = std::env::var("ROUTER_MAX_CALL_RECORDS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100_000);

    let usage_log_dir = std::env::var("ROUTER_USAGE_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("data/usage"));

    let usage_log_dir = normalize_usage_log_dir(usage_log_dir);

    let existing_records = load_call_records_from_disk(&usage_log_dir, max_call_records).await;

    let state = AppState {
        cfg_path,
        usage_log_dir,
        cfg: Arc::new(RwLock::new(cfg)),
        rr_index: Arc::new(AtomicUsize::new(0)),
        group_rr_index: Arc::new(RwLock::new(HashMap::new())),
        http_client: Client::builder()
            .pool_idle_timeout(std::time::Duration::from_secs(60))
            .tcp_keepalive(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(8))
            .build()
            .context("failed to build reqwest client")?,
        upstream_timeout_secs,
        call_records: Arc::new(RwLock::new(existing_records)),
        max_call_records,
    };

    let app = Router::new()
        .route("/", get(root_redirect))
        .route("/healthz", get(healthz))
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(proxy::proxy_chat_completions))
        .route("/v1/embeddings", post(proxy::proxy_embeddings))
        .route("/admin/login", post(admin::admin_login_handler))
        .route("/admin/targets", get(admin::admin_list_targets).post(admin::admin_create_target))
        .route(
            "/admin/model-groups",
            get(admin::admin_list_model_groups).post(admin::admin_create_model_group),
        )
        .route(
            "/admin/targets/:id",
            put(admin::admin_update_target).delete(admin::admin_delete_target),
        )
        .route(
            "/admin/model-groups/:id",
            put(admin::admin_update_model_group).delete(admin::admin_delete_model_group),
        )
        .route("/admin/test-target/:id", get(admin::admin_test_target))
        .route("/admin/stats", get(admin::admin_get_stats))
        .nest_service("/ui", ServeDir::new("ui"))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let bind_addr: SocketAddr = std::env::var("ROUTER_BIND")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
        .parse()
        .context("invalid ROUTER_BIND")?;

    info!("llm-router listening on {}", bind_addr);
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn healthz() -> impl IntoResponse {
    Json(json!({"status": "ok"}))
}

async fn root_redirect() -> Redirect {
    Redirect::temporary("/ui")
}

async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    let cfg = state.cfg.read().await;
    let mut data: Vec<Value> = cfg
        .targets
        .iter()
        .filter(|t| t.enabled)
        .map(|t| {
            json!({
                "id": t.router_model,
                "object": "model",
                "created": Utc::now().timestamp(),
                "owned_by": "llm-router"
            })
        })
        .collect();

    let mut group_items: Vec<Value> = cfg
        .model_groups
        .iter()
        .filter(|g| g.enabled)
        .map(|g| {
            json!({
                "id": g.name,
                "object": "model",
                "created": Utc::now().timestamp(),
                "owned_by": "router-group"
            })
        })
        .collect();
    data.append(&mut group_items);

    Json(json!({
        "object": "list",
        "data": data
    }))
}
