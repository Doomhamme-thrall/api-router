use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tracing::{debug, info, warn};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_quota: Option<TokenQuota>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelGroup {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub target_ids: Vec<String>,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_quota: Option<TokenQuota>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenQuota {
    pub limit: u64,
    pub window_seconds: u64,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_quota: Option<TokenQuota>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpsertModelGroupRequest {
    pub name: String,
    #[serde(default)]
    pub target_ids: Vec<String>,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_quota: Option<TokenQuota>,
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
    debug!("loading config from {}", path.display());
    let body = match fs::read_to_string(path).await {
        Ok(v) => v,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            warn!(
                "config file not found at {}, generating default config",
                path.display()
            );
            let cfg = generate_default_config();
            info!(
                "default config generated: admin/admin, jwt_secret={}, empty targets/groups",
                cfg.jwt_secret
            );
            save_config(path, &cfg).await?;
            return Ok(cfg);
        }
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read config from {}", path.display()));
        }
    };
    let cfg: RouterConfig = serde_json::from_str(&body)
        .with_context(|| format!("invalid config json at {}", path.display()))?;
    debug!("config loaded successfully ({} bytes)", body.len());
    Ok(cfg)
}

pub async fn save_config(path: &Path, cfg: &RouterConfig) -> anyhow::Result<()> {
    debug!("saving config to {}", path.display());
    let body = serde_json::to_string_pretty(cfg)?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, body).await?;
    fs::rename(tmp_path, path).await?;
    debug!("config saved to {}", path.display());
    Ok(())
}

/// Generate a default config with admin/admin credentials and a random JWT secret.
/// Useful when the config file does not exist on first run.
fn generate_default_config() -> RouterConfig {
    let admin = AdminConfig {
        username: "admin".to_string(),
        password_sha256: sha256_hex("admin"),
    };
    let jwt_secret = uuid::Uuid::new_v4().to_string();
    RouterConfig {
        admin,
        jwt_secret,
        client_api_keys: Vec::new(),
        targets: Vec::new(),
        model_groups: Vec::new(),
    }
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let out = hasher.finalize();
    hex::encode(out)
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
