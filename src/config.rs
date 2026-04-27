use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    pub admin: AdminConfig,
    pub jwt_secret: String,
    #[serde(default)]
    pub client_api_keys: Vec<String>,
    #[serde(default)]
    pub targets: Vec<UpstreamTarget>,
    #[serde(default)]
    pub model_groups: Vec<ModelGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminConfig {
    pub username: String,
    pub password_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamTarget {
    pub id: String,
    pub name: String,
    #[serde(default = "default_api_format")]
    pub api_format: String,
    pub base_url: String,
    pub api_key: String,
    pub router_model: String,
    pub upstream_model: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelGroup {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub target_ids: Vec<String>,
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpsertTargetRequest {
    pub name: String,
    #[serde(default = "default_api_format")]
    pub api_format: String,
    pub base_url: String,
    pub api_key: String,
    pub router_model: String,
    pub upstream_model: String,
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpsertModelGroupRequest {
    pub name: String,
    #[serde(default)]
    pub target_ids: Vec<String>,
    pub enabled: bool,
}

fn default_api_format() -> String {
    "openai".to_string()
}

pub fn normalize_api_format(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "gemini" => "gemini".to_string(),
        _ => "openai".to_string(),
    }
}

pub fn is_gemini_format(value: &str) -> bool {
    normalize_api_format(value) == "gemini"
}

pub fn normalize_usage_log_dir(input: PathBuf) -> PathBuf {
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

pub async fn load_config(path: &Path) -> anyhow::Result<RouterConfig> {
    let body = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read config from {}", path.display()))?;
    let cfg: RouterConfig = serde_json::from_str(&body)
        .with_context(|| format!("invalid config json at {}", path.display()))?;
    Ok(cfg)
}

pub async fn save_config(path: &Path, cfg: &RouterConfig) -> anyhow::Result<()> {
    let body = serde_json::to_string_pretty(cfg)?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, body).await?;
    fs::rename(tmp_path, path).await?;
    Ok(())
}

pub fn build_upstream_url(base_url: &str, route: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let route = route.trim_start_matches('/');

    let is_full_endpoint = (route == "chat/completions" && base.ends_with("/chat/completions"))
        || (route == "embeddings" && base.ends_with("/embeddings"));
    if is_full_endpoint {
        return base.to_string();
    }

    // If base already ends with /v1, don't add another /v1
    if base.ends_with("/v1") || base.ends_with("/v1/") {
        return format!("{}/{}", base, route);
    }

    if base.ends_with("/api/v3") {
        return format!("{}/{}", base, route);
    }

    format!("{}/v1/{}", base, route)
}
