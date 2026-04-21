use std::collections::{HashMap, HashSet};

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use tracing::error;

use crate::auth::{admin_login, api_error, require_admin, AdminLoginRequest};
use crate::config::{
    is_gemini_format, normalize_api_format, build_upstream_url, save_config,
    UpsertModelGroupRequest, UpsertTargetRequest,
};
use crate::gemini::{build_gemini_request_payload, build_gemini_upstream_url};
use crate::state::AppState;
use crate::usage::{aggregate_usage_from_disk, apply_record_to_agg, StatsQuery, TargetStats};

pub async fn admin_login_handler(
    State(state): State<AppState>,
    Json(req): Json<AdminLoginRequest>,
) -> Response {
    match admin_login(&state, &req).await {
        Ok(resp) => Json(resp).into_response(),
        Err(resp) => resp,
    }
}

pub async fn admin_list_targets(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let cfg = state.cfg.read().await;
    Json(json!({"items": cfg.targets})).into_response()
}

pub async fn admin_create_target(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<UpsertTargetRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let new_target = crate::config::UpstreamTarget {
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

pub async fn admin_update_target(
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

pub async fn admin_delete_target(
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

pub async fn admin_list_model_groups(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let cfg = state.cfg.read().await;
    Json(json!({"items": cfg.model_groups})).into_response()
}

pub async fn admin_create_model_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<UpsertModelGroupRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let new_group = crate::config::ModelGroup {
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

pub async fn admin_update_model_group(
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

pub async fn admin_delete_model_group(
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

pub async fn admin_test_target(
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

pub async fn admin_get_stats(
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
