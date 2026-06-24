use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;

use axum::body::{Body, Bytes};
use axum::extract::{State, Json};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use chrono::Utc;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, info, warn};

use crate::auth::{api_error, validate_client_api_key};
use crate::config::{is_gemini_format, build_upstream_url};
use crate::gemini::{
    build_gemini_request_payload, build_gemini_upstream_url, build_gemini_streaming_url,
    build_openai_sse_from_completion, gemini_to_openai_chat_completion, gemini_chunk_to_openai_sse,
};
use crate::state::AppState;
use crate::usage::{
    append_call_record_to_disk, append_group_usage_to_disk, extract_tokens_from_bytes,
    extract_tokens_from_sse_bytes, extract_tokens_from_value, sum_group_tokens_in_window,
    CallRecord, GroupUsageRecord,
};

/// 将生成的助手内容追加到消息列表中，以便进行续写
fn append_assistant_content(payload: &mut Value, content: &str) {
    if content.is_empty() {
        return;
    }
    if let Some(messages) = payload.get_mut("messages").and_then(|v| v.as_array_mut()) {
        if let Some(last) = messages.last_mut() {
            if last.get("role").and_then(|v| v.as_str()) == Some("assistant") {
                if let Some(old_content) = last.get_mut("content") {
                    if let Some(s) = old_content.as_str() {
                        *old_content = Value::String(format!("{}{}", s, content));
                        return;
                    }
                }
            }
        }
        messages.push(json!({"role": "assistant", "content": content}));
    }
}

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
    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let model_name = payload.get("model").and_then(|v| v.as_str()).unwrap_or("unknown");
    let is_streaming = payload.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);

    info!(
        "proxy request: {} model={} stream={} route={}",
        client_ip, model_name, is_streaming, route,
    );

    if let Err(resp) = validate_client_api_key(&state, &headers).await {
        warn!("api key validation failed from {}", client_ip);
        return resp;
    }
    debug!("api key validated for request model={}", model_name);

    let requested_model = payload
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let candidates = pick_target_candidates_for_request(&state, requested_model.as_deref()).await;
    if candidates.is_empty() {
        warn!("no candidates found for model={}", model_name);
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "no enabled upstream target for requested route",
        );
    }

    let mut last_err_message = String::from("upstream request failed");

    // For streaming OpenAI-compatible requests, inject stream_options.include_usage
    // so that the upstream returns token usage in the final SSE chunk.
    if is_streaming {
        if payload.get("stream_options").is_none() {
            payload["stream_options"] = json!({"include_usage": true});
        }
    }

    for target in candidates {
        debug!("trying target: {} (upstream_model={})", target.name, target.upstream_model);
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

            let upstream_url = if is_streaming {
                build_gemini_streaming_url(&target.base_url, &target.upstream_model)
            } else {
                build_gemini_upstream_url(&target.base_url, &target.upstream_model)
            };
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

            if !is_streaming {
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

                record_call(&state, &target, true, pt, ct, tt).await;
                record_group_usage(&state, requested_model.as_deref(), tt).await;
                info!("gemini success: target={} tokens={} (pt={}, ct={})", target.name, tt, pt, ct);
                let mut response = Response::new(Body::from(openai_like.to_string()));
                *response.status_mut() = status;
                response.headers_mut().insert(
                    "content-type",
                    HeaderValue::from_static("application/json"),
                );
                return response;
            } else {
                // Gemini Streaming with Auto-Resume
                let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(100);
                let state_clone = state.clone();
                let target_clone = target.clone();
                let requested_model_clone = requested_model.clone();
                let mut current_openai_payload = attempt_payload.clone();
                
                let mut first_resp = Some(upstream_resp);

                tokio::spawn(async move {
                    let mut total_prompt_tokens = 0;
                    let mut total_completion_tokens = 0;
                    let mut last_was_done = false;
                    let id = format!("chatcmpl-gemini-{}", Utc::now().timestamp_millis());
                    let created = Utc::now().timestamp();

                    for retry in 0..3 {
                        let resp = if let Some(r) = first_resp.take() {
                            r
                        } else {
                            // Re-build Gemini payload from updated OpenAI payload
                            let gemini_payload = match build_gemini_request_payload(&current_openai_payload) {
                                Ok(v) => v,
                                Err(err) => {
                                    error!("gemini resume payload build failed: {}", err);
                                    break;
                                }
                            };
                            let upstream_url = build_gemini_streaming_url(&target_clone.base_url, &target_clone.upstream_model);
                            let req = state_clone
                                .http_client
                                .post(upstream_url)
                                .header("Content-Type", "application/json")
                                .timeout(std::time::Duration::from_secs(state_clone.upstream_timeout_secs))
                                .query(&[("key", target_clone.api_key.as_str())])
                                .json(&gemini_payload);
                            
                            match req.send().await {
                                Ok(r) if r.status().is_success() => r,
                                Ok(r) => {
                                    warn!("gemini resume retry {} failed with status {}", retry, r.status());
                                    break;
                                }
                                Err(e) => {
                                    warn!("gemini resume retry {} failed: {}", retry, e);
                                    break;
                                }
                            }
                        };

                        let mut stream = resp.bytes_stream();
                        let mut buffer = Vec::new();
                        let mut current_attempt_text = String::new();
                        let mut attempt_done = false;

                        while let Some(chunk_res) = stream.next().await {
                            match chunk_res {
                                Ok(bytes) => {
                                    buffer.extend_from_slice(&bytes);
                                    
                                    loop {
                                        // Skip whitespace/array-syntax
                                        let mut start_idx = 0;
                                        while start_idx < buffer.len() && ((buffer[start_idx] as char).is_whitespace() || buffer[start_idx] == b'[' || buffer[start_idx] == b',') {
                                            start_idx += 1;
                                        }
                                        if start_idx > 0 { buffer.drain(0..start_idx); }
                                        if buffer.is_empty() || buffer[0] == b']' { break; }

                                        let mut brace_count = 0;
                                        let mut end_idx = None;
                                        let mut in_string = false;
                                        let mut escaped = false;
                                        for (i, &b) in buffer.iter().enumerate() {
                                            let c = b as char;
                                            if escaped { escaped = false; continue; }
                                            if c == '\\' { escaped = true; continue; }
                                            if c == '"' { in_string = !in_string; continue; }
                                            if !in_string {
                                                if c == '{' { brace_count += 1; }
                                                else if c == '}' { brace_count -= 1; if brace_count == 0 { end_idx = Some(i + 1); break; } }
                                            }
                                        }

                                        if let Some(i) = end_idx {
                                            let obj_bytes = buffer.drain(0..i).collect::<Vec<u8>>();
                                            if let Ok(v) = serde_json::from_slice::<Value>(&obj_bytes) {
                                                // Extract text for resume
                                                if let Some(candidates) = v.get("candidates").and_then(|c| c.as_array()) {
                                                    if let Some(cand) = candidates.first() {
                                                        if let Some(parts) = cand.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
                                                            for p in parts {
                                                                if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                                                                    current_attempt_text.push_str(t);
                                                                }
                                                            }
                                                        }
                                                        if let Some(reason) = cand.get("finishReason").and_then(|v| v.as_str()) {
                                                            if reason == "STOP" {
                                                                attempt_done = true;
                                                            }
                                                        }
                                                    }
                                                }
                                                // Extract usage
                                                if let Some(usage) = v.get("usageMetadata") {
                                                    total_prompt_tokens = usage.get("promptTokenCount").and_then(|v| v.as_u64()).unwrap_or(total_prompt_tokens);
                                                    total_completion_tokens += usage.get("candidatesTokenCount").and_then(|v| v.as_u64()).unwrap_or(0);
                                                }

                                                let (sse_text, _) = gemini_chunk_to_openai_sse(&v, &target_clone.upstream_model, &id, created);
                                                if tx.send(Ok(Bytes::from(sse_text))).await.is_err() {
                                                    return;
                                                }
                                            }
                                        } else {
                                            break;
                                        }
                                    }
                                }
                                Err(err) => {
                                    warn!("gemini stream error during retry {}: {}", retry, err);
                                    break;
                                }
                            }
                        }

                        if attempt_done {
                            last_was_done = true;
                            break;
                        } else {
                            info!("gemini stream truncated, attempting resume (retry {})", retry + 1);
                            append_assistant_content(&mut current_openai_payload, &current_attempt_text);
                        }
                    }

                    let _ = tx.send(Ok(Bytes::from("data: [DONE]\n\n"))).await;

                    let tt = total_prompt_tokens + total_completion_tokens;
                    record_call(&state_clone, &target_clone, true, total_prompt_tokens, total_completion_tokens, tt).await;
                    record_group_usage(&state_clone, requested_model_clone.as_deref(), tt).await;
                    info!("gemini success (streaming with resume): target={} tokens={}", target_clone.name, tt);
                });

                let body = Body::from_stream(ReceiverStream::new(rx));
                let mut response = Response::new(body);
                *response.status_mut() = status;
                response.headers_mut().insert(
                    "content-type",
                    HeaderValue::from_static("text/event-stream"),
                );
                return response;
            }
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

        } else {
            let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(100);
            let state_clone = state.clone();
            let target_clone = target.clone();
            let requested_model_clone = requested_model.clone();
            let mut current_payload = attempt_payload.clone();
            let route_clone = route.to_string();
            let content_type_clone = content_type.clone();
            
            let mut first_resp = Some(upstream_resp);

            tokio::spawn(async move {
                let mut total_prompt_tokens = 0;
                let mut total_completion_tokens = 0;
                let mut last_was_done = false;

                for retry in 0..3 {
                    let resp = if let Some(r) = first_resp.take() {
                        r
                    } else {
                        let upstream_url = build_upstream_url(&target_clone.base_url, &route_clone);
                        let req = state_clone
                            .http_client
                            .post(upstream_url)
                            .header("Authorization", format!("Bearer {}", target_clone.api_key))
                            .header("Content-Type", "application/json")
                            .timeout(std::time::Duration::from_secs(state_clone.upstream_timeout_secs))
                            .json(&current_payload);
                        
                        match req.send().await {
                            Ok(r) if r.status().is_success() => r,
                            Ok(r) => {
                                warn!("openai resume retry {} failed with status {}", retry, r.status());
                                break;
                            }
                            Err(e) => {
                                warn!("openai resume retry {} failed: {}", retry, e);
                                break;
                            }
                        }
                    };

                    let mut stream = resp.bytes_stream();
                    let mut current_attempt_text = String::new();
                    let mut attempt_done = false;

                    while let Some(chunk_res) = stream.next().await {
                        match chunk_res {
                            Ok(bytes) => {
                                if let Ok(text) = std::str::from_utf8(&bytes) {
                                    for line in text.lines() {
                                        let line = line.trim();
                                        if line.is_empty() { continue; }
                                        let data = line.strip_prefix("data: ").unwrap_or(line).trim();
                                        if data == "[DONE]" {
                                            attempt_done = true;
                                            continue;
                                        }
                                        if let Ok(v) = serde_json::from_str::<Value>(data) {
                                            if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
                                                if let Some(choice) = choices.first() {
                                                    if let Some(delta) = choice.get("delta") {
                                                        if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                                                            current_attempt_text.push_str(content);
                                                        }
                                                    }
                                                    if let Some(reason) = choice.get("finish_reason").filter(|r| !r.is_null()) {
                                                        // Stop if it's "stop". If it's "length", we might want to continue.
                                                        if reason == "stop" {
                                                            attempt_done = true;
                                                        }
                                                    }
                                                }
                                            }
                                            if let Some(usage) = v.get("usage") {
                                                total_prompt_tokens = usage.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(total_prompt_tokens);
                                                total_completion_tokens += usage.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                            }
                                        }
                                    }
                                }
                                if tx.send(Ok(bytes)).await.is_err() {
                                    return; // Client disconnected
                                }
                            }
                            Err(err) => {
                                warn!("openai stream error during retry {}: {}", retry, err);
                                break;
                            }
                        }
                    }

                    if attempt_done {
                        last_was_done = true;
                        break;
                    } else {
                        info!("openai stream truncated, attempting resume (retry {})", retry + 1);
                        append_assistant_content(&mut current_payload, &current_attempt_text);
                    }
                }

                if !last_was_done {
                    let _ = tx.send(Ok(Bytes::from("data: [DONE]\n\n"))).await;
                }

                let tt = total_prompt_tokens + total_completion_tokens;
                record_call(&state_clone, &target_clone, true, total_prompt_tokens, total_completion_tokens, tt).await;
                record_group_usage(&state_clone, requested_model_clone.as_deref(), tt).await;
                info!("openai success (streaming with resume): target={} tokens={}", target_clone.name, tt);
            });

            let body = Body::from_stream(ReceiverStream::new(rx));
            let mut response = Response::new(body);
            *response.status_mut() = status;
            response.headers_mut().insert("content-type", content_type_clone);
            return response;
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

/// Check if a target has a token quota that has been exceeded.
fn is_target_quota_exceeded(
    target: &crate::config::UpstreamTarget,
    usage: &HashMap<String, VecDeque<GroupUsageRecord>>,
    now: i64,
) -> bool {
    let Some(ref quota) = target.token_quota else { return false };
    let Some(records) = usage.get(&target.id) else { return false };
    let total = sum_group_tokens_in_window(records, now, quota.window_seconds);
    if total >= quota.limit {
        warn!(
            "target '{}' quota exceeded: {}/{} tokens in {}s window",
            target.name, total, quota.limit, quota.window_seconds,
        );
        return true;
    }
    false
}

async fn pick_global_target_candidates(state: &AppState) -> Vec<crate::config::UpstreamTarget> {
    let cfg = state.cfg.read().await;
    let now = Utc::now().timestamp();
    let usage = state.target_usage.read().await;
    let enabled: Vec<crate::config::UpstreamTarget> = cfg.targets
        .iter()
        .filter(|t| t.enabled && !is_target_quota_exceeded(t, &usage, now))
        .cloned()
        .collect();
    drop(usage);
    drop(cfg);
    if enabled.is_empty() {
        return Vec::new();
    }

    let idx = state.rr_index.fetch_add(1, Ordering::Relaxed);
    rotate_targets(enabled, idx)
}

async fn model_group_exists(state: &AppState, group_name: &str) -> bool {
    let cfg = state.cfg.read().await;
    let Some(group) = cfg.model_groups.iter().find(|g| g.enabled && g.name == group_name) else {
        debug!("group name='{}' not found or disabled", group_name);
        return false;
    };
    // If group has a quota, check if it's exceeded
    if let Some(ref quota) = group.token_quota {
        let now = Utc::now().timestamp();
        let usage = state.group_usage.read().await;
        let records = usage.get(&group.id);
        if let Some(records) = records {
            let total = sum_group_tokens_in_window(records, now, quota.window_seconds);
            if total >= quota.limit {
                warn!(
                    "group '{}' quota exceeded: {}/{} tokens in {}s window",
                    group.name, total, quota.limit, quota.window_seconds,
                );
                return false; // quota exceeded, treat group as non-existent
            }
            debug!(
                "group '{}' quota: {}/{} tokens used",
                group.name, total, quota.limit,
            );
        }
    }
    debug!("group '{}' resolved successfully ({}/{} targets)", group.name, group.target_ids.len(), group.target_ids.len());
    true
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
    let now = Utc::now().timestamp();
    let target_usage = state.target_usage.read().await;
    let candidates: Vec<crate::config::UpstreamTarget> = cfg
        .targets
        .iter()
        .filter(|t| {
            t.enabled
                && selected_ids.contains(t.id.as_str())
                && !is_target_quota_exceeded(t, &target_usage, now)
        })
        .cloned()
        .collect();
    drop(target_usage);
    drop(cfg);

    if candidates.is_empty() {
        warn!("group '{}' has no enabled targets among its {} members", group_name, group_target_ids.len());
        return Vec::new();
    }

    let mut rr_map = state.group_rr_index.write().await;
    let counter = rr_map.entry(group_id).or_insert(0);
    let idx = *counter;
    *counter = counter.wrapping_add(1);
    let rr_idx = idx % candidates.len();

    debug!(
        "group '{}' rr_idx={}/{} candidates: {}",
        group_name,
        rr_idx,
        candidates.len(),
        candidates.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", "),
    );

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

    // Also record target-level token quota usage if applicable
    record_target_usage(state, &target.id, &target.name, total_tokens).await;
}

async fn record_target_usage(
    state: &AppState,
    target_id: &str,
    target_name: &str,
    total_tokens: u64,
) {
    // Check if this target has a quota configured before recording
    let has_quota = {
        let cfg = state.cfg.read().await;
        cfg.targets
            .iter()
            .any(|t| t.id == target_id && t.token_quota.is_some())
    };
    if !has_quota || total_tokens == 0 {
        return;
    }

    let now = Utc::now().timestamp();
    let record = GroupUsageRecord {
        group_id: target_id.to_string(), // reuse field name for persistence
        timestamp: now,
        total_tokens,
    };

    {
        let mut usage = state.target_usage.write().await;
        usage
            .entry(target_id.to_string())
            .or_insert_with(VecDeque::new)
            .push_back(record.clone());
    }

    debug!(
        "target usage recorded: target={} tokens={}",
        target_name, total_tokens,
    );

    if let Err(err) = append_group_usage_to_disk(&state.target_usage_log_dir, &record).await {
        error!("failed to append target usage record to disk: {}", err);
    }
}

/// Record token usage for a group (if the request was routed through a group).
/// Deduces the group from the requested model name.
async fn record_group_usage(
    state: &AppState,
    requested_model: Option<&str>,
    total_tokens: u64,
) {
    let Some(model_name) = requested_model else { return };
    if total_tokens == 0 { return; }

    let group_id = {
        let cfg = state.cfg.read().await;
        cfg.model_groups
            .iter()
            .find(|g| g.enabled && g.name == model_name)
            .map(|g| g.id.clone())
    };
    let Some(group_id) = group_id else { return };

    let now = Utc::now().timestamp();
    let record = GroupUsageRecord {
        group_id: group_id.clone(),
        timestamp: now,
        total_tokens,
    };

    {
        let mut usage = state.group_usage.write().await;
        usage.entry(group_id.clone())
            .or_insert_with(VecDeque::new)
            .push_back(record.clone());
    }

    debug!("group usage recorded: group_id={} tokens={}", group_id, total_tokens);

    if let Err(err) = append_group_usage_to_disk(&state.group_usage_log_dir, &record).await {
        error!("failed to append group usage record to disk: {}", err);
    }
}
