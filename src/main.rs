use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use anyhow::Context;
use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{Duration, Utc};
use futures_util::StreamExt;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::{fs, sync::RwLock};
use tower_http::{cors::CorsLayer, services::ServeDir, trace::TraceLayer};
use tracing::{error, info};

#[derive(Clone)]
struct AppState {
    cfg_path: PathBuf,
    cfg: Arc<RwLock<RouterConfig>>,
    rr_index: Arc<AtomicUsize>,
    group_rr_index: Arc<RwLock<HashMap<String, usize>>>,
    http_client: Client,
    upstream_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RouterConfig {
    admin: AdminConfig,
    jwt_secret: String,
    #[serde(default)]
    client_api_keys: Vec<String>,
    #[serde(default)]
    targets: Vec<UpstreamTarget>,
    #[serde(default)]
    model_groups: Vec<ModelGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AdminConfig {
    username: String,
    password_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UpstreamTarget {
    id: String,
    name: String,
    provider: String,
    base_url: String,
    api_key: String,
    router_model: String,
    upstream_model: String,
    enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelGroup {
    id: String,
    name: String,
    #[serde(default)]
    target_ids: Vec<String>,
    enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct AdminLoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AdminLoginResponse {
    token: String,
    expires_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
    iat: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct UpsertTargetRequest {
    name: String,
    provider: String,
    base_url: String,
    api_key: String,
    router_model: String,
    upstream_model: String,
    enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct UpsertModelGroupRequest {
    name: String,
    #[serde(default)]
    target_ids: Vec<String>,
    enabled: bool,
}

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

    let state = AppState {
        cfg_path,
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
    };

    let app = Router::new()
        .route("/", get(root_redirect))
        .route("/healthz", get(healthz))
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(proxy_chat_completions))
        .route("/v1/embeddings", post(proxy_embeddings))
        .route("/admin/login", post(admin_login))
        .route("/admin/targets", get(admin_list_targets).post(admin_create_target))
        .route(
            "/admin/model-groups",
            get(admin_list_model_groups).post(admin_create_model_group),
        )
        .route(
            "/admin/targets/:id",
            put(admin_update_target).delete(admin_delete_target),
        )
        .route(
            "/admin/model-groups/:id",
            put(admin_update_model_group).delete(admin_delete_model_group),
        )
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

async fn load_config(path: &Path) -> anyhow::Result<RouterConfig> {
    let body = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read config from {}", path.display()))?;
    let cfg: RouterConfig = serde_json::from_str(&body)
        .with_context(|| format!("invalid config json at {}", path.display()))?;
    Ok(cfg)
}

async fn save_config(path: &Path, cfg: &RouterConfig) -> anyhow::Result<()> {
    let body = serde_json::to_string_pretty(cfg)?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, body).await?;
    fs::rename(tmp_path, path).await?;
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
                "owned_by": t.provider
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

async fn proxy_chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut payload): Json<Value>,
) -> Response {
    proxy_openai_request(state, headers, "chat/completions", &mut payload).await
}

async fn proxy_embeddings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut payload): Json<Value>,
) -> Response {
    proxy_openai_request(state, headers, "embeddings", &mut payload).await
}

async fn proxy_openai_request(
    state: AppState,
    headers: HeaderMap,
    route: &str,
    payload: &mut Value,
) -> Response {
    if let Err(resp) = validate_client_api_key(&state, &headers).await {
        return resp;
    }

    let requested_model = payload
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let candidates = pick_target_candidates_for_request(&state, requested_model.as_deref()).await;
    if candidates.is_empty() {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "no enabled upstream target for requested route",
        );
    }

    let mut last_err_message = String::from("upstream request failed");

    for target in candidates {
        let mut attempt_payload = payload.clone();
        attempt_payload["model"] = Value::String(target.upstream_model.clone());

        let upstream_url = build_upstream_url(&target.base_url, route);
        let req = state
            .http_client
            .post(upstream_url)
            .header("Authorization", format!("Bearer {}", target.api_key))
            .header("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(state.upstream_timeout_secs))
            .json(&attempt_payload);

        let upstream_resp = match req.send().await {
            Ok(resp) => resp,
            Err(err) => {
                last_err_message = format!("upstream request failed: {}", err);
                error!("{}", last_err_message);
                continue;
            }
        };

        let status = upstream_resp.status();
        if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
            last_err_message = format!("upstream status {}", status);
            error!("{}", last_err_message);
            continue;
        }

        let content_type = upstream_resp
            .headers()
            .get("content-type")
            .cloned()
            .unwrap_or_else(|| HeaderValue::from_static("application/json"));

        let stream = upstream_resp.bytes_stream();
        let body = Body::from_stream(stream.map(|chunk| match chunk {
            Ok(bytes) => Ok(bytes),
            Err(err) => {
                error!("streaming error: {}", err);
                Err(std::io::Error::new(std::io::ErrorKind::Other, "stream read failed"))
            }
        }));

        let mut response = Response::new(body);
        *response.status_mut() = status;
        response
            .headers_mut()
            .insert("content-type", content_type);
        return response;
    }

    api_error(StatusCode::BAD_GATEWAY, &last_err_message)
}

async fn validate_client_api_key(
    state: &AppState,
    headers: &HeaderMap,
) -> std::result::Result<(), Response> {
    let cfg = state.cfg.read().await;
    if cfg.client_api_keys.is_empty() {
        return Ok(());
    }

    let Some(auth) = headers.get("authorization") else {
        return Err(api_error(StatusCode::UNAUTHORIZED, "missing authorization header"));
    };

    let Ok(auth) = auth.to_str() else {
        return Err(api_error(StatusCode::UNAUTHORIZED, "invalid authorization header"));
    };

    let supplied = auth.strip_prefix("Bearer ").unwrap_or("").trim();
    let ok = cfg.client_api_keys.iter().any(|k| k == supplied);
    if ok {
        Ok(())
    } else {
        Err(api_error(StatusCode::UNAUTHORIZED, "invalid api key"))
    }
}

fn rotate_targets(candidates: Vec<UpstreamTarget>, start: usize) -> Vec<UpstreamTarget> {
    let len = candidates.len();
    if len == 0 {
        return Vec::new();
    }
    let offset = start % len;
    candidates
        .iter()
        .cycle()
        .skip(offset)
        .take(len)
        .cloned()
        .collect()
}

async fn pick_global_target_candidates(state: &AppState) -> Vec<UpstreamTarget> {
    let cfg = state.cfg.read().await;
    let enabled: Vec<UpstreamTarget> = cfg.targets.iter().filter(|t| t.enabled).cloned().collect();
    if enabled.is_empty() {
        return Vec::new();
    }

    let idx = state.rr_index.fetch_add(1, Ordering::Relaxed);
    rotate_targets(enabled, idx)
}

async fn pick_target_candidates_for_request(
    state: &AppState,
    requested_model: Option<&str>,
) -> Vec<UpstreamTarget> {
    if let Some(group_name) = requested_model {
        if model_group_exists(state, group_name).await {
            return pick_target_candidates_from_group(state, group_name).await;
        }
    }
    pick_global_target_candidates(state).await
}

async fn model_group_exists(state: &AppState, group_name: &str) -> bool {
    let cfg = state.cfg.read().await;
    cfg.model_groups
        .iter()
        .any(|g| g.enabled && g.name == group_name)
}

async fn pick_target_candidates_from_group(state: &AppState, group_name: &str) -> Vec<UpstreamTarget> {
    let cfg = state.cfg.read().await;
    let group = cfg
        .model_groups
        .iter()
        .find(|g| g.enabled && g.name == group_name);
    let Some(group) = group else {
        return Vec::new();
    };
    let group_id = group.id.clone();
    let group_target_ids = group.target_ids.clone();

    let selected_ids: HashSet<&str> = group_target_ids.iter().map(String::as_str).collect();
    let candidates: Vec<UpstreamTarget> = cfg
        .targets
        .iter()
        .filter(|t| t.enabled && selected_ids.contains(t.id.as_str()))
        .cloned()
        .collect();
    drop(cfg);

    if candidates.is_empty() {
        return Vec::new();
    }

    let mut rr_map = state.group_rr_index.write().await;
    let counter = rr_map.entry(group_id).or_insert(0);
    let idx = *counter;
    *counter = counter.wrapping_add(1);

    rotate_targets(candidates, idx)
}

fn api_error(status: StatusCode, message: &str) -> Response {
    let body = Json(json!({
        "error": {
            "message": message,
            "type": "router_error"
        }
    }));
    (status, body).into_response()
}

fn build_upstream_url(base_url: &str, route: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let route = route.trim_start_matches('/');

    let is_full_endpoint = (route == "chat/completions" && base.ends_with("/chat/completions"))
        || (route == "embeddings" && base.ends_with("/embeddings"));
    if is_full_endpoint {
        return base.to_string();
    }

    if base.ends_with("/api/v3") {
        return format!("{}/{}", base, route);
    }

    format!("{}/v1/{}", base, route)
}

async fn admin_login(
    State(state): State<AppState>,
    Json(req): Json<AdminLoginRequest>,
) -> Response {
    let cfg = state.cfg.read().await;
    let password_hash = sha256_hex(&req.password);
    if req.username != cfg.admin.username || password_hash != cfg.admin.password_sha256 {
        return api_error(StatusCode::UNAUTHORIZED, "invalid credentials");
    }

    let now = Utc::now();
    let exp = now + Duration::hours(12);
    let claims = Claims {
        sub: cfg.admin.username.clone(),
        iat: now.timestamp() as usize,
        exp: exp.timestamp() as usize,
    };

    let token = match encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(cfg.jwt_secret.as_bytes()),
    ) {
        Ok(token) => token,
        Err(err) => {
            error!("failed to encode jwt: {}", err);
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to create token");
        }
    };

    Json(AdminLoginResponse {
        token,
        expires_at: exp.timestamp(),
    })
    .into_response()
}

async fn admin_list_targets(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let cfg = state.cfg.read().await;
    Json(json!({"items": cfg.targets})).into_response()
}

async fn admin_create_target(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<UpsertTargetRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let new_target = UpstreamTarget {
        id: uuid::Uuid::new_v4().to_string(),
        name: req.name,
        provider: req.provider,
        base_url: req.base_url,
        api_key: req.api_key,
        router_model: req.router_model,
        upstream_model: req.upstream_model,
        enabled: req.enabled,
    };

    {
        let mut cfg = state.cfg.write().await;
        cfg.targets.push(new_target.clone());
        if let Err(err) = save_config(&state.cfg_path, &cfg).await {
            error!("failed to save config: {}", err);
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to persist config");
        }
    }

    Json(new_target).into_response()
}

async fn admin_update_target(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    Json(req): Json<UpsertTargetRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    {
        let mut cfg = state.cfg.write().await;
        let target = cfg.targets.iter_mut().find(|t| t.id == id);
        let Some(target) = target else {
            return api_error(StatusCode::NOT_FOUND, "target not found");
        };

        target.name = req.name;
        target.provider = req.provider;
        target.base_url = req.base_url;
        target.api_key = req.api_key;
        target.router_model = req.router_model;
        target.upstream_model = req.upstream_model;
        target.enabled = req.enabled;

        if let Err(err) = save_config(&state.cfg_path, &cfg).await {
            error!("failed to save config: {}", err);
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to persist config");
        }
    }

    Json(json!({"ok": true})).into_response()
}

async fn admin_delete_target(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    {
        let mut cfg = state.cfg.write().await;
        let old_len = cfg.targets.len();
        cfg.targets.retain(|t| t.id != id);
        if cfg.targets.len() == old_len {
            return api_error(StatusCode::NOT_FOUND, "target not found");
        }
        if let Err(err) = save_config(&state.cfg_path, &cfg).await {
            error!("failed to save config: {}", err);
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to persist config");
        }
    }

    Json(json!({"ok": true})).into_response()
}

async fn admin_list_model_groups(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let cfg = state.cfg.read().await;
    Json(json!({"items": cfg.model_groups})).into_response()
}

async fn admin_create_model_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<UpsertModelGroupRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let new_group = ModelGroup {
        id: uuid::Uuid::new_v4().to_string(),
        name: req.name,
        target_ids: req.target_ids,
        enabled: req.enabled,
    };

    {
        let mut cfg = state.cfg.write().await;
        if cfg.model_groups.iter().any(|g| g.name == new_group.name) {
            return api_error(StatusCode::CONFLICT, "model group name already exists");
        }

        let target_id_set: HashSet<&str> = cfg.targets.iter().map(|t| t.id.as_str()).collect();
        if !new_group
            .target_ids
            .iter()
            .all(|id| target_id_set.contains(id.as_str()))
        {
            return api_error(StatusCode::BAD_REQUEST, "model group contains unknown target id");
        }

        cfg.model_groups.push(new_group.clone());
        if let Err(err) = save_config(&state.cfg_path, &cfg).await {
            error!("failed to save config: {}", err);
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to persist config");
        }
    }

    Json(new_group).into_response()
}

async fn admin_update_model_group(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    Json(req): Json<UpsertModelGroupRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    {
        let mut cfg = state.cfg.write().await;
        if cfg
            .model_groups
            .iter()
            .any(|g| g.name == req.name && g.id != id)
        {
            return api_error(StatusCode::CONFLICT, "model group name already exists");
        }

        let target_id_set: HashSet<&str> = cfg.targets.iter().map(|t| t.id.as_str()).collect();
        if !req
            .target_ids
            .iter()
            .all(|target_id| target_id_set.contains(target_id.as_str()))
        {
            return api_error(StatusCode::BAD_REQUEST, "model group contains unknown target id");
        }

        let group = cfg.model_groups.iter_mut().find(|g| g.id == id);
        let Some(group) = group else {
            return api_error(StatusCode::NOT_FOUND, "model group not found");
        };

        group.name = req.name;
        group.target_ids = req.target_ids;
        group.enabled = req.enabled;

        if let Err(err) = save_config(&state.cfg_path, &cfg).await {
            error!("failed to save config: {}", err);
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to persist config");
        }
    }

    Json(json!({"ok": true})).into_response()
}

async fn admin_delete_model_group(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    {
        let mut cfg = state.cfg.write().await;
        let old_len = cfg.model_groups.len();
        cfg.model_groups.retain(|g| g.id != id);
        if cfg.model_groups.len() == old_len {
            return api_error(StatusCode::NOT_FOUND, "model group not found");
        }
        if let Err(err) = save_config(&state.cfg_path, &cfg).await {
            error!("failed to save config: {}", err);
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to persist config");
        }
    }

    {
        let mut rr = state.group_rr_index.write().await;
        rr.remove(&id);
    }

    Json(json!({"ok": true})).into_response()
}

async fn require_admin(
    state: &AppState,
    headers: &HeaderMap,
) -> std::result::Result<(), Response> {
    let Some(auth) = headers.get("authorization") else {
        return Err(api_error(StatusCode::UNAUTHORIZED, "missing authorization header"));
    };
    let Ok(auth) = auth.to_str() else {
        return Err(api_error(StatusCode::UNAUTHORIZED, "invalid authorization header"));
    };

    let token = auth.strip_prefix("Bearer ").unwrap_or("").trim();
    if token.is_empty() {
        return Err(api_error(StatusCode::UNAUTHORIZED, "missing token"));
    }

    let cfg = state.cfg.read().await;
    let result = decode::<Claims>(
        token,
        &DecodingKey::from_secret(cfg.jwt_secret.as_bytes()),
        &Validation::default(),
    );

    match result {
        Ok(_) => Ok(()),
        Err(_) => Err(api_error(StatusCode::UNAUTHORIZED, "invalid token")),
    }
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let out = hasher.finalize();
    hex::encode(out)
}
