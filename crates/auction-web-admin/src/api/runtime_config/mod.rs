//! Admin runtime 配置 HTTP 客户端。
//! 浏览器只编辑非敏感 JSON 文档，并把服务端返回的 fingerprint 原样作为更新 fence；TOML
//! 文件、密钥和命令账本永远不进入浏览器。

mod document;
mod fields;
mod infrastructure;
mod operations;
mod taxonomy;
use self::document::valid_runtime_document;
use self::fields::{invalid_runtime_response, valid_fingerprint};

pub(crate) use auction_contracts::AdminRuntimeConfigResponse as AdminRuntimeConfig;
use auction_contracts::{AdminRuntimeConfigUpdateRequest, AdminRuntimeConfigValidateRequest};
use auction_web_shared::wire::json_exact_object;
use gloo_net::http::Request;
use serde_json::Value;

use super::{
    AdminApiFailure, ApiResponse, invalid_request_error, invalid_response_error, response_failure,
    send_get, send_request,
};

/// Runtime 配置请求失败时保留 HTTP 状态，页面可区分 fingerprint 冲突和普通错误。
pub(crate) type AdminRuntimeConfigApiError = AdminApiFailure;

pub(crate) async fn fetch_admin_runtime_config()
-> Result<AdminRuntimeConfig, AdminRuntimeConfigApiError> {
    let response = send_get("/api/v1/admin/config?include_equipment_taxonomy=true")
        .await
        .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<AdminRuntimeConfig> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    decode_runtime_config(payload.data, false).map_err(invalid_runtime_response)
}

pub(crate) async fn validate_admin_runtime_config(
    document: Value,
) -> Result<AdminRuntimeConfig, AdminRuntimeConfigApiError> {
    let csrf_token =
        super::fetch_csrf_token().await.map_err(|code| AdminApiFailure { status: None, code })?;
    let command = AdminRuntimeConfigValidateRequest { document };
    let response = send_request(|abort_signal| {
        Request::post("/api/v1/admin/config:validate?include_equipment_taxonomy=true")
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<AdminRuntimeConfig> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    decode_runtime_config(payload.data, false).map_err(invalid_runtime_response)
}

pub(crate) async fn update_admin_runtime_config(
    command_id: uuid::Uuid,
    expected_fingerprint: String,
    document: Value,
) -> Result<AdminRuntimeConfig, AdminRuntimeConfigApiError> {
    let csrf_token =
        super::fetch_csrf_token().await.map_err(|code| AdminApiFailure { status: None, code })?;
    let command = AdminRuntimeConfigUpdateRequest { command_id, expected_fingerprint, document };
    let response = send_request(|abort_signal| {
        Request::put("/api/v1/admin/config?include_equipment_taxonomy=true")
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<AdminRuntimeConfig> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    decode_runtime_config(payload.data, true).map_err(invalid_runtime_response)
}

fn decode_runtime_config(
    value: AdminRuntimeConfig,
    expected_restart_required: bool,
) -> Result<AdminRuntimeConfig, &'static str> {
    if value.restart_required != expected_restart_required
        || !valid_fingerprint(&value.fingerprint)
        || !valid_runtime_document(&value.document)
        || !value.equipment_taxonomy.as_ref().is_some_and(taxonomy::valid_equipment_taxonomy)
    {
        return Err("runtime_config");
    }
    Ok(value)
}
