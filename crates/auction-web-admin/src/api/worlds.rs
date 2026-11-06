//! Admin 数据库权威业务世界客户端。

use std::collections::HashSet;

use auction_contracts::{
    AdminWorldListResponse, AdminWorldResponse, AdminWorldSetEnabledRequest,
    AdminWorldSetEnabledResponse,
};
use gloo_net::http::Request;
use uuid::Uuid;

use super::{
    AdminApiFailure, ApiResponse, MAX_JSON_SAFE_INTEGER, fetch_csrf_token, invalid_request_error,
    invalid_response_error, response_error, response_failure, send_get, send_request,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdminBusinessWorld {
    pub id: String,
    pub enabled: bool,
    pub availability: String,
    pub policy_revision: i64,
}

// 结构闭集由 contracts 的 `AdminWorldResponse`/`AdminWorldListResponse`/
// `AdminWorldSetEnabledResponse`（`deny_unknown_fields`）负责；本文件只保留世界枚举、
// 数量与排序等跨字段谓词。

/// 读取数据库权威的业务世界状态；它与重启生效的 Worker 入站配置保持独立。
pub(crate) async fn fetch_admin_business_worlds() -> Result<Vec<AdminBusinessWorld>, String> {
    let response = send_get("/api/v1/admin/worlds").await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminWorldListResponse> =
        response.json().await.map_err(invalid_response_error)?;
    decode_worlds(payload.data.worlds).map_err(invalid_response_error)
}

/// 提交带 revision fence 和稳定 command ID 的世界启停命令。
pub(crate) async fn set_admin_business_world_enabled(
    command_id: Uuid,
    world_id: String,
    enabled: bool,
    expected_policy_revision: i64,
) -> Result<AdminBusinessWorld, AdminApiFailure> {
    if !matches!(world_id.as_str(), "main" | "isekai")
        || expected_policy_revision <= 0
        || expected_policy_revision > MAX_JSON_SAFE_INTEGER
    {
        return Err(AdminApiFailure::client("invalid_request"));
    }
    let csrf_token = fetch_csrf_token().await.map_err(AdminApiFailure::client)?;
    let request = AdminWorldSetEnabledRequest { command_id, enabled, expected_policy_revision };
    let url = format!("/api/v1/admin/worlds/{world_id}");
    let response = send_request(|abort_signal| {
        Request::patch(&url)
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&request)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<AdminWorldSetEnabledResponse> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    let world = decode_world(payload.data.world)
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    if world.id != world_id
        || world.enabled != enabled
        || !matches!(
            world.policy_revision,
            revision if revision == expected_policy_revision
                || revision == expected_policy_revision.saturating_add(1)
        )
    {
        return Err(AdminApiFailure::client("invalid_response"));
    }
    Ok(world)
}

fn decode_worlds(values: Vec<AdminWorldResponse>) -> Result<Vec<AdminBusinessWorld>, &'static str> {
    if values.len() != 2 {
        return Err("business_world_count");
    }
    let mut ids = HashSet::with_capacity(values.len());
    let mut worlds = values
        .into_iter()
        .map(|value| {
            let world = decode_world(value)?;
            if !ids.insert(world.id.clone()) {
                return Err("business_world_duplicate");
            }
            Ok(world)
        })
        .collect::<Result<Vec<_>, _>>()?;
    worlds.sort_by_key(|world| if world.id == "main" { 0 } else { 1 });
    Ok(worlds)
}

fn decode_world(value: AdminWorldResponse) -> Result<AdminBusinessWorld, &'static str> {
    if !matches!(value.id.as_str(), "main" | "isekai")
        || !matches!(value.availability.as_str(), "available" | "degraded" | "unavailable")
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.policy_revision)
    {
        return Err("business_world");
    }
    Ok(AdminBusinessWorld {
        id: value.id,
        enabled: value.enabled,
        availability: value.availability,
        policy_revision: value.policy_revision,
    })
}
