//! Admin runtime 配置 HTTP 客户端。
//!
//! 浏览器只编辑非敏感 JSON 文档，并把服务端返回的 fingerprint 原样作为更新 fence；TOML
//! 文件、密钥和命令账本永远不进入浏览器。

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::ApiResponse;

/// Runtime 配置请求失败时保留 HTTP 状态，页面可区分 fingerprint 冲突和普通错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdminRuntimeConfigApiError {
    pub status: Option<u16>,
    pub message: String,
}

impl AdminRuntimeConfigApiError {
    /// 判断服务端是否拒绝了过期 fingerprint。
    pub(crate) const fn is_conflict(&self) -> bool {
        matches!(self.status, Some(409))
    }
}

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

pub(crate) async fn fetch_admin_runtime_config()
-> Result<AdminRuntimeConfig, AdminRuntimeConfigApiError> {
    let response = Request::get("/api/v1/admin/config").send().await.map_err(request_error)?;
    if !response.ok() {
        return Err(response_error(response.status()));
    }
    let payload: ApiResponse<AdminRuntimeConfig> =
        response.json().await.map_err(response_body_error)?;
    Ok(payload.data)
}

pub(crate) async fn validate_admin_runtime_config(
    document: Value,
) -> Result<AdminRuntimeConfig, AdminRuntimeConfigApiError> {
    let csrf_token = super::fetch_csrf_token().await.map_err(message_error)?;
    let request = Request::post("/api/v1/admin/config:validate")
        .header("x-csrf-token", &csrf_token)
        .json(&AdminRuntimeConfigValidateRequest { document })
        .map_err(request_error)?;
    let response = request.send().await.map_err(request_error)?;
    if !response.ok() {
        return Err(response_error(response.status()));
    }
    let payload: ApiResponse<AdminRuntimeConfig> =
        response.json().await.map_err(response_body_error)?;
    Ok(payload.data)
}

pub(crate) async fn update_admin_runtime_config(
    command_id: uuid::Uuid,
    expected_fingerprint: String,
    document: Value,
) -> Result<AdminRuntimeConfig, AdminRuntimeConfigApiError> {
    let csrf_token = super::fetch_csrf_token().await.map_err(message_error)?;
    let request = Request::put("/api/v1/admin/config")
        .header("x-csrf-token", &csrf_token)
        .json(&AdminRuntimeConfigUpdateRequest { command_id, expected_fingerprint, document })
        .map_err(request_error)?;
    let response = request.send().await.map_err(request_error)?;
    if !response.ok() {
        return Err(response_error(response.status()));
    }
    let payload: ApiResponse<AdminRuntimeConfig> =
        response.json().await.map_err(response_body_error)?;
    Ok(payload.data)
}

fn request_error(error: impl ToString) -> AdminRuntimeConfigApiError {
    message_error(error.to_string())
}

fn response_body_error(error: impl ToString) -> AdminRuntimeConfigApiError {
    message_error(error.to_string())
}

fn response_error(status: u16) -> AdminRuntimeConfigApiError {
    AdminRuntimeConfigApiError { status: Some(status), message: format!("HTTP {status}") }
}

fn message_error(message: String) -> AdminRuntimeConfigApiError {
    AdminRuntimeConfigApiError { status: None, message }
}
