//! Admin runtime 配置 HTTP 客户端。
//!
//! 浏览器只编辑非敏感 JSON 文档，并把服务端返回的 fingerprint 原样作为更新 fence；TOML
//! 文件、密钥和命令账本永远不进入浏览器。

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::ApiResponse;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminRuntimeConfig {
    pub document: Value,
    pub fingerprint: String,
    pub restart_required: bool,
}

#[derive(Debug, Clone, Serialize)]
struct AdminRuntimeConfigValidateRequest {
    document: Value,
}

#[derive(Debug, Clone, Serialize)]
struct AdminRuntimeConfigUpdateRequest {
    command_id: Uuid,
    expected_fingerprint: String,
    document: Value,
}

pub(crate) async fn fetch_admin_runtime_config() -> Result<AdminRuntimeConfig, String> {
    let response =
        Request::get("/api/v1/admin/config").send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminRuntimeConfig> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn validate_admin_runtime_config(
    document: Value,
) -> Result<AdminRuntimeConfig, String> {
    let csrf_token = super::fetch_csrf_token().await?;
    let request = Request::post("/api/v1/admin/config:validate")
        .header("x-csrf-token", &csrf_token)
        .json(&AdminRuntimeConfigValidateRequest { document })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminRuntimeConfig> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn update_admin_runtime_config(
    expected_fingerprint: String,
    document: Value,
) -> Result<AdminRuntimeConfig, String> {
    let csrf_token = super::fetch_csrf_token().await?;
    let request = Request::put("/api/v1/admin/config")
        .header("x-csrf-token", &csrf_token)
        .json(&AdminRuntimeConfigUpdateRequest {
            command_id: Uuid::new_v4(),
            expected_fingerprint,
            document,
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminRuntimeConfig> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}
