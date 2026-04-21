use std::collections::HashSet;
use std::sync::atomic::Ordering;

use axum::body::Body;
use axum::extract::{State, Json};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use chrono::Utc;
use serde_json::{json, Value};
use tracing::error;

use crate::auth::{api_error, validate_client_api_key};
use crate::config::{is_gemini_format, build_upstream_url};
use crate::gemini::{
    build_gemini_request_payload, build_gemini_upstream_url,
    build_openai_sse_from_completion, gemini_to_openai_chat_completion,
};
use crate::state::AppState;
use crate::usage::{
    append_call_record_to_disk, extract_tokens_from_bytes, extract_tokens_from_sse_bytes,
    extract_tokens_from_value, CallRecord,
};

pub async fn proxy_chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut payload): Json<Value>,
) -> Response {
    proxy_openai_request(state, headers, "chat/completions", &mut payload).await
}

pub async fn proxy_embeddings(
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

    // For streaming OpenAI-compatible requests, inject stream_options.include_usage
    // so that the upstream returns token usage in the final SSE chunk.
    if is_streaming {
        if payload.get("stream_options").is_none() {
            payload["stream_options"] = json!({"include_usage": true});
        }
    }

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

        // OpenAI-compatible path
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
            // For streaming responses, read the full body to extract token usage,
            // then replay the SSE stream to the client.
            match upstream_resp.bytes().await {
                Ok(bytes) => {
                    let (pt, ct, tt) = extract_tokens_from_sse_bytes(&bytes);
                    record_call(&state, &target, true, pt, ct, tt).await;
                    let body = Body::from(bytes);
                    let mut response = Response::new(body);
                    *response.status_mut() = status;
                    response
                        .headers_mut()
                        .insert("content-type", content_type);
                    return response;
                }
                Err(err) => {
                    last_err_message = format!("failed to read streaming response body: {}", err);
                    error!("{}", last_err_message);
                    record_call(&state, &target, false, 0, 0, 0).await;
                    continue;
                }
            }
        }
    }

    api_error(StatusCode::BAD_GATEWAY, &last_err_message)
}

// --- target selection / round-robin ---

fn rotate_targets(candidates: Vec<crate::config::UpstreamTarget>, start: usize) -> Vec<crate::config::UpstreamTarget> {
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

async fn pick_global_target_candidates(state: &AppState) -> Vec<crate::config::UpstreamTarget> {
    let cfg = state.cfg.read().await;
    let enabled: Vec<crate::config::UpstreamTarget> = cfg.targets.iter().filter(|t| t.enabled).cloned().collect();
    if enabled.is_empty() {
        return Vec::new();
    }

    let idx = state.rr_index.fetch_add(1, Ordering::Relaxed);
    rotate_targets(enabled, idx)
}

async fn model_group_exists(state: &AppState, group_name: &str) -> bool {
    let cfg = state.cfg.read().await;
    cfg.model_groups
        .iter()
        .any(|g| g.enabled && g.name == group_name)
}

async fn pick_target_candidates_from_group(state: &AppState, group_name: &str) -> Vec<crate::config::UpstreamTarget> {
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
    let candidates: Vec<crate::config::UpstreamTarget> = cfg
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

async fn pick_target_candidates_for_request(
    state: &AppState,
    requested_model: Option<&str>,
) -> Vec<crate::config::UpstreamTarget> {
    if let Some(group_name) = requested_model {
        if model_group_exists(state, group_name).await {
            return pick_target_candidates_from_group(state, group_name).await;
        }
    }
    pick_global_target_candidates(state).await
}

// --- call recording ---

async fn record_call(
    state: &AppState,
    target: &crate::config::UpstreamTarget,
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
