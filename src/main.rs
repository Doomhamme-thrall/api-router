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
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{Duration, TimeZone, Utc};
use futures_util::StreamExt;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::{fs, sync::RwLock};
use tokio::io::AsyncWriteExt;
use tower_http::{cors::CorsLayer, services::ServeDir, trace::TraceLayer};
use tracing::{error, info};

#[derive(Clone)]
struct AppState {
    cfg_path: PathBuf,
    usage_log_dir: PathBuf,
    cfg: Arc<RwLock<RouterConfig>>,
    rr_index: Arc<AtomicUsize>,
    group_rr_index: Arc<RwLock<HashMap<String, usize>>>,
    http_client: Client,
    upstream_timeout_secs: u64,
    call_records: Arc<RwLock<Vec<CallRecord>>>,
    max_call_records: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CallRecord {
    target_id: String,
    target_name: String,
    timestamp: i64,
    success: bool,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

#[derive(Debug, Serialize)]
struct TargetStats {
    target_id: String,
    target_name: String,
    total_calls: u64,
    success_count: u64,
    error_count: u64,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct StatsQuery {
    from: Option<i64>,
    to: Option<i64>,
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
    #[serde(default = "default_api_format")]
    api_format: String,
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
    #[serde(default = "default_api_format")]
    api_format: String,
    base_url: String,
    api_key: String,
    router_model: String,
    upstream_model: String,
    enabled: bool,
}

fn default_api_format() -> String {
    "openai".to_string()
}

fn normalize_api_format(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "gemini" => "gemini".to_string(),
        _ => "openai".to_string(),
    }
}

fn is_gemini_format(value: &str) -> bool {
    normalize_api_format(value) == "gemini"
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
        .route("/admin/test-target/:id", get(admin_test_target))
        .route("/admin/stats", get(admin_get_stats))
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

    let is_streaming = payload.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut last_err_message = String::from("upstream request failed");

    for target in candidates {
        let mut attempt_payload = payload.clone();
        attempt_payload["model"] = Value::String(target.upstream_model.clone());

        if is_gemini_format(&target.api_format) {
            if route != "chat/completions" {
                last_err_message = "gemini target only supports chat/completions".to_string();
                error!("{}", last_err_message);
                record_call(&state, &target, false, 0, 0, 0).await;
                continue;
            }

            let gemini_payload = match build_gemini_request_payload(&attempt_payload) {
                Ok(v) => v,
                Err(err) => {
                    last_err_message = err;
                    error!("{}", last_err_message);
                    record_call(&state, &target, false, 0, 0, 0).await;
                    continue;
                }
            };

            let upstream_url = build_gemini_upstream_url(&target.base_url, &target.upstream_model);
            let req = state
                .http_client
                .post(upstream_url)
                .header("Content-Type", "application/json")
                .timeout(std::time::Duration::from_secs(state.upstream_timeout_secs))
                .query(&[("key", target.api_key.as_str())])
                .json(&gemini_payload);

            let upstream_resp = match req.send().await {
                Ok(resp) => resp,
                Err(err) => {
                    last_err_message = format!("upstream request failed: {}", err);
                    error!("{}", last_err_message);
                    record_call(&state, &target, false, 0, 0, 0).await;
                    continue;
                }
            };

            let status = upstream_resp.status();
            if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
                last_err_message = format!("upstream status {}", status);
                error!("{}", last_err_message);
                record_call(&state, &target, false, 0, 0, 0).await;
                continue;
            }

            let body_bytes = match upstream_resp.bytes().await {
                Ok(bytes) => bytes,
                Err(err) => {
                    last_err_message = format!("failed to read response body: {}", err);
                    error!("{}", last_err_message);
                    record_call(&state, &target, false, 0, 0, 0).await;
                    continue;
                }
            };

            if !status.is_success() {
                let mut response = Response::new(Body::from(body_bytes));
                *response.status_mut() = status;
                response.headers_mut().insert(
                    "content-type",
                    HeaderValue::from_static("application/json"),
                );
                return response;
            }

            let gemini_body: Value = match serde_json::from_slice(&body_bytes) {
                Ok(v) => v,
                Err(err) => {
                    last_err_message = format!("invalid gemini response json: {}", err);
                    error!("{}", last_err_message);
                    record_call(&state, &target, false, 0, 0, 0).await;
                    continue;
                }
            };

            let openai_like = gemini_to_openai_chat_completion(&gemini_body, &target.upstream_model);
            let (pt, ct, tt) = extract_tokens_from_value(&openai_like);

            if !is_streaming {
                record_call(&state, &target, true, pt, ct, tt).await;
                let mut response = Response::new(Body::from(openai_like.to_string()));
                *response.status_mut() = status;
                response.headers_mut().insert(
                    "content-type",
                    HeaderValue::from_static("application/json"),
                );
                return response;
            }

            let sse_body = build_openai_sse_from_completion(&openai_like);
            record_call(&state, &target, true, pt, ct, tt).await;
            let mut response = Response::new(Body::from(sse_body));
            *response.status_mut() = status;
            response.headers_mut().insert(
                "content-type",
                HeaderValue::from_static("text/event-stream"),
            );
            return response;
        }

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
                record_call(&state, &target, false, 0, 0, 0).await;
                continue;
            }
        };

        let status = upstream_resp.status();
        if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
            last_err_message = format!("upstream status {}", status);
            error!("{}", last_err_message);
            record_call(&state, &target, false, 0, 0, 0).await;
            continue;
        }

        let content_type = upstream_resp
            .headers()
            .get("content-type")
            .cloned()
            .unwrap_or_else(|| HeaderValue::from_static("application/json"));

        if !is_streaming {
            match upstream_resp.bytes().await {
                Ok(bytes) => {
                    let (pt, ct, tt) = extract_tokens_from_bytes(&bytes);
                    record_call(&state, &target, true, pt, ct, tt).await;
                    let body = Body::from(bytes);
                    let mut response = Response::new(body);
                    *response.status_mut() = status;
                    response.headers_mut().insert("content-type", content_type);
                    return response;
                }
                Err(err) => {
                    last_err_message = format!("failed to read response body: {}", err);
                    error!("{}", last_err_message);
                    record_call(&state, &target, false, 0, 0, 0).await;
                    continue;
                }
            }
        } else {
            record_call(&state, &target, true, 0, 0, 0).await;
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

async fn record_call(
    state: &AppState,
    target: &UpstreamTarget,
    success: bool,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
) {
    let record = CallRecord {
        target_id: target.id.clone(),
        target_name: target.name.clone(),
        timestamp: Utc::now().timestamp(),
        success,
        prompt_tokens,
        completion_tokens,
        total_tokens,
    };
    {
        let mut records = state.call_records.write().await;
        records.push(record.clone());
        if records.len() > state.max_call_records {
            let drain = records.len() - state.max_call_records;
            records.drain(0..drain);
        }
    }

    if let Err(err) = append_call_record_to_disk(&state.usage_log_dir, &record).await {
        error!("failed to append usage record to disk: {}", err);
    }
}

fn normalize_usage_log_dir(input: PathBuf) -> PathBuf {
    if input
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
    {
        return input
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("data"));
    }

    input
}

fn day_key_from_timestamp(ts: i64) -> String {
    Utc.timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "1970-01-01".to_string())
}

fn usage_log_file_for_day(dir: &Path, day_key: &str) -> PathBuf {
    dir.join(format!("usage-{}.jsonl", day_key))
}

fn usage_log_file_for_timestamp(dir: &Path, ts: i64) -> PathBuf {
    usage_log_file_for_day(dir, &day_key_from_timestamp(ts))
}

async fn list_all_usage_log_files(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut items = Vec::new();
    let mut rd = match fs::read_dir(dir).await {
        Ok(v) => v,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(items),
        Err(err) => return Err(err.into()),
    };

    while let Some(entry) = rd.next_entry().await? {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with("usage-") && name.ends_with(".jsonl") {
            items.push(path);
        }
    }

    items.sort();
    Ok(items)
}

fn try_day_key(ts: i64) -> Option<String> {
    Utc.timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
}

fn day_keys_in_range(from: i64, to: i64) -> Option<Vec<String>> {
    let start = try_day_key(from)?;
    let end = try_day_key(to)?;
    let mut day = chrono::NaiveDate::parse_from_str(&start, "%Y-%m-%d").ok()?;
    let end_day = chrono::NaiveDate::parse_from_str(&end, "%Y-%m-%d").ok()?;
    if day > end_day {
        return Some(Vec::new());
    }

    let mut keys = Vec::new();
    while day <= end_day {
        keys.push(day.format("%Y-%m-%d").to_string());
        day = match day.succ_opt() {
            Some(v) => v,
            None => break,
        };
    }
    Some(keys)
}

async fn usage_log_files_for_range(dir: &Path, from: i64, to: i64) -> anyhow::Result<Vec<PathBuf>> {
    if from <= 0 || to == i64::MAX {
        return list_all_usage_log_files(dir).await;
    }

    let Some(day_keys) = day_keys_in_range(from, to) else {
        return list_all_usage_log_files(dir).await;
    };
    let mut files = Vec::new();
    for day_key in day_keys {
        let path = usage_log_file_for_day(dir, &day_key);
        if fs::metadata(&path).await.is_ok() {
            files.push(path);
        }
    }
    Ok(files)
}

async fn append_call_record_to_disk(dir: &Path, record: &CallRecord) -> anyhow::Result<()> {
    fs::create_dir_all(dir).await?;

    let path = usage_log_file_for_timestamp(dir, record.timestamp);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await?;
    let line = serde_json::to_string(record)?;
    file.write_all(line.as_bytes()).await?;
    file.write_all(b"\n").await?;
    Ok(())
}

async fn load_call_records_from_disk(dir: &Path, max_records: usize) -> Vec<CallRecord> {
    let files = match list_all_usage_log_files(dir).await {
        Ok(v) => v,
        Err(err) => {
            error!("failed to list usage logs in {}: {}", dir.display(), err);
            return Vec::new();
        }
    };

    let mut items = Vec::new();
    for path in files {
        let body = match fs::read_to_string(&path).await {
            Ok(v) => v,
            Err(err) => {
                error!("failed to read usage log from {}: {}", path.display(), err);
                continue;
            }
        };

        for (idx, line) in body.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<CallRecord>(line) {
                Ok(record) => items.push(record),
                Err(err) => error!(
                    "invalid usage log line {} in {}: {}",
                    idx + 1,
                    path.display(),
                    err
                ),
            }
        }
    }

    if items.len() > max_records {
        let drop_count = items.len() - max_records;
        items.drain(0..drop_count);
    }
    items
}

fn apply_record_to_agg(record: &CallRecord, agg: &mut HashMap<String, TargetStats>) {
    let entry = agg.entry(record.target_id.clone()).or_insert(TargetStats {
        target_id: record.target_id.clone(),
        target_name: record.target_name.clone(),
        total_calls: 0,
        success_count: 0,
        error_count: 0,
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: 0,
    });
    entry.total_calls += 1;
    if record.success {
        entry.success_count += 1;
    } else {
        entry.error_count += 1;
    }
    entry.prompt_tokens += record.prompt_tokens;
    entry.completion_tokens += record.completion_tokens;
    entry.total_tokens += record.total_tokens;
}

async fn aggregate_usage_from_disk(
    dir: &Path,
    from: i64,
    to: i64,
) -> anyhow::Result<HashMap<String, TargetStats>> {
    let files = usage_log_files_for_range(dir, from, to).await?;
    let mut agg: HashMap<String, TargetStats> = HashMap::new();
    for path in files {
        let body = match fs::read_to_string(&path).await {
            Ok(v) => v,
            Err(err) => {
                error!("failed to read usage log from {}: {}", path.display(), err);
                continue;
            }
        };

        for line in body.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(record) = serde_json::from_str::<CallRecord>(line) {
                if record.timestamp >= from && record.timestamp <= to {
                    apply_record_to_agg(&record, &mut agg);
                }
            }
        }
    }
    Ok(agg)
}

fn extract_tokens_from_bytes(bytes: &[u8]) -> (u64, u64, u64) {
    if let Ok(v) = serde_json::from_slice::<Value>(bytes) {
        if let Some(usage) = v.get("usage") {
            let prompt = usage.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let completion = usage.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let total = usage.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            return (prompt, completion, total);
        }
    }
    (0, 0, 0)
}

fn extract_tokens_from_value(v: &Value) -> (u64, u64, u64) {
    let usage = v.get("usage").and_then(|v| v.as_object());
    let prompt = usage
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let completion = usage
        .and_then(|u| u.get("completion_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let total = usage
        .and_then(|u| u.get("total_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    (prompt, completion, total)
}

fn message_content_to_text(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(items) = content.as_array() {
        let mut texts = Vec::new();
        for item in items {
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                texts.push(text.to_string());
            }
        }
        return texts.join("\n");
    }
    String::new()
}

fn build_gemini_request_payload(openai_payload: &Value) -> Result<Value, String> {
    let Some(messages) = openai_payload.get("messages").and_then(|v| v.as_array()) else {
        return Err("gemini target requires openai messages[]".to_string());
    };

    let mut system_parts = Vec::new();
    let mut contents = Vec::new();
    for msg in messages {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
        let text = msg
            .get("content")
            .map(message_content_to_text)
            .unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }

        if role == "system" {
            system_parts.push(json!({"text": text}));
            continue;
        }

        let gemini_role = if role == "assistant" { "model" } else { "user" };
        contents.push(json!({
            "role": gemini_role,
            "parts": [{"text": text}]
        }));
    }

    if contents.is_empty() {
        return Err("gemini target requires at least one non-system message".to_string());
    }

    let mut out = json!({"contents": contents});

    let mut generation_config = serde_json::Map::new();
    if let Some(v) = openai_payload.get("temperature") {
        generation_config.insert("temperature".to_string(), v.clone());
    }
    if let Some(v) = openai_payload.get("top_p") {
        generation_config.insert("topP".to_string(), v.clone());
    }
    if let Some(v) = openai_payload.get("max_tokens") {
        generation_config.insert("maxOutputTokens".to_string(), v.clone());
    }
    if let Some(v) = openai_payload.get("stop") {
        generation_config.insert("stopSequences".to_string(), v.clone());
    }

    if !generation_config.is_empty() {
        out["generationConfig"] = Value::Object(generation_config);
    }

    if !system_parts.is_empty() {
        out["systemInstruction"] = json!({"parts": system_parts});
    }

    Ok(out)
}

fn build_gemini_upstream_url(base_url: &str, model: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.contains(":generateContent") {
        return base.to_string();
    }
    if base.contains("/models/") {
        if base.contains(':') {
            return base.to_string();
        }
        return format!("{}:generateContent", base);
    }
    format!("{}/v1beta/models/{}:generateContent", base, model)
}

fn map_gemini_finish_reason(reason: &str) -> &str {
    match reason {
        "STOP" => "stop",
        "MAX_TOKENS" => "length",
        "SAFETY" => "content_filter",
        "RECITATION" => "content_filter",
        _ => "stop",
    }
}

fn gemini_to_openai_chat_completion(gemini_body: &Value, model: &str) -> Value {
    let text = gemini_body
        .get("candidates")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|cand| cand.get("content"))
        .and_then(|content| content.get("parts"))
        .and_then(|v| v.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    let finish_reason = gemini_body
        .get("candidates")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|cand| cand.get("finishReason"))
        .and_then(|v| v.as_str())
        .map(map_gemini_finish_reason)
        .unwrap_or("stop");

    let prompt_tokens = gemini_body
        .get("usageMetadata")
        .and_then(|u| u.get("promptTokenCount"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let completion_tokens = gemini_body
        .get("usageMetadata")
        .and_then(|u| u.get("candidatesTokenCount"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let total_tokens = gemini_body
        .get("usageMetadata")
        .and_then(|u| u.get("totalTokenCount"))
        .and_then(|v| v.as_u64())
        .unwrap_or(prompt_tokens + completion_tokens);

    json!({
        "id": format!("chatcmpl-gemini-{}", Utc::now().timestamp_millis()),
        "object": "chat.completion",
        "created": Utc::now().timestamp(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": text
            },
            "finish_reason": finish_reason
        }],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": total_tokens
        }
    })
}

fn build_openai_sse_from_completion(completion: &Value) -> String {
    let id = completion
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("chatcmpl-gemini");
    let created = completion
        .get("created")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| Utc::now().timestamp());
    let model = completion
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("gemini");
    let text = completion
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let first = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": Value::Null}]
    });
    let second = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": "stop"}]
    });

    format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        first,
        second
    )
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
        api_format: normalize_api_format(&req.api_format),
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
        target.api_format = normalize_api_format(&req.api_format);
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

async fn admin_test_target(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let target = {
        let cfg = state.cfg.read().await;
        cfg.targets.iter().find(|t| t.id == id).cloned()
    };

    let Some(target) = target else {
        return api_error(StatusCode::NOT_FOUND, "target not found");
    };

    let payload = json!({
        "model": target.upstream_model,
        "messages": [{"role": "user", "content": "Say hi."}],
        "max_tokens": 50,
        "stream": false
    });

    let result = if is_gemini_format(&target.api_format) {
        let gemini_payload = match build_gemini_request_payload(&payload) {
            Ok(v) => v,
            Err(err) => {
                return Json(json!({"ok": false, "error": err})).into_response();
            }
        };
        let upstream_url = build_gemini_upstream_url(&target.base_url, &target.upstream_model);
        state
            .http_client
            .post(&upstream_url)
            .header("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(30))
            .query(&[("key", target.api_key.as_str())])
            .json(&gemini_payload)
            .send()
            .await
    } else {
        let upstream_url = build_upstream_url(&target.base_url, "chat/completions");
        state
            .http_client
            .post(&upstream_url)
            .header("Authorization", format!("Bearer {}", target.api_key))
            .header("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(30))
            .json(&payload)
            .send()
            .await
    };

    match result {
        Err(err) => Json(json!({
            "ok": false,
            "error": format!("request failed: {}", err)
        }))
        .into_response(),
        Ok(resp) => {
            let status = resp.status();
            match resp.json::<Value>().await {
                Err(err) => Json(json!({
                    "ok": false,
                    "error": format!("failed to read response: {}", err)
                }))
                .into_response(),
                Ok(body) => {
                    if status.is_success() {
                        Json(json!({"ok": true, "response": body})).into_response()
                    } else {
                        Json(json!({"ok": false, "error": body})).into_response()
                    }
                }
            }
        }
    }
}

async fn admin_get_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<StatsQuery>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let from = query.from.unwrap_or(0);
    let to = query.to.unwrap_or(i64::MAX);

    let agg = match aggregate_usage_from_disk(&state.usage_log_dir, from, to).await {
        Ok(v) => v,
        Err(err) => {
            error!(
                "failed to aggregate usage log from disk ({}), fallback to memory: {}",
                state.usage_log_dir.display(),
                err
            );
            let records = state.call_records.read().await;
            let mut in_memory_agg: HashMap<String, TargetStats> = HashMap::new();
            for record in records.iter().filter(|r| r.timestamp >= from && r.timestamp <= to) {
                apply_record_to_agg(record, &mut in_memory_agg);
            }
            in_memory_agg
        }
    };

    let mut stats: Vec<TargetStats> = agg.into_values().collect();
    stats.sort_by(|a, b| a.target_name.cmp(&b.target_name));

    Json(json!({"items": stats})).into_response()
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let out = hasher.finalize();
    hex::encode(out)
}
