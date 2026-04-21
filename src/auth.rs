use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::state::AppState;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
    pub iat: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminLoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminLoginResponse {
    pub token: String,
    pub expires_at: i64,
}

pub fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let out = hasher.finalize();
    hex::encode(out)
}

pub fn api_error(status: StatusCode, message: &str) -> Response {
    let body = Json(json!({
        "error": {
            "message": message,
            "type": "router_error"
        }
    }));
    (status, body).into_response()
}

pub async fn validate_client_api_key(
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

pub async fn require_admin(
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

pub async fn admin_login(
    state: &AppState,
    req: &AdminLoginRequest,
) -> Result<AdminLoginResponse, Response> {
    let cfg = state.cfg.read().await;
    let password_hash = sha256_hex(&req.password);
    if req.username != cfg.admin.username || password_hash != cfg.admin.password_sha256 {
        return Err(api_error(StatusCode::UNAUTHORIZED, "invalid credentials"));
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
            tracing::error!("failed to encode jwt: {}", err);
            return Err(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create token",
            ));
        }
    };

    Ok(AdminLoginResponse {
        token,
        expires_at: exp.timestamp(),
    })
}
