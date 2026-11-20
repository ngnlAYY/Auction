//! Admin 数据库权威业务世界客户端。

use std::collections::HashSet;

use auction_contracts::{
    AdminWorldListResponse, AdminWorldResponse, AdminWorldSetEnabledRequest,
    AdminWorldSetEnabledResponse, ApiResponse,
};
use gloo_net::http::Method;
use uuid::Uuid;

use super::AdminWriteFailure;
use super::transport::WriteRequest;
use super::{MAX_JSON_SAFE_INTEGER, invalid_response_error, response_error, send_get};

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
) -> Result<AdminBusinessWorld, AdminWriteFailure> {
    if !matches!(world_id.as_str(), "main" | "isekai")
        || expected_policy_revision <= 0
        || expected_policy_revision > MAX_JSON_SAFE_INTEGER
    {
        return Err(AdminWriteFailure::not_sent("invalid_request"));
    }
    let request = AdminWorldSetEnabledRequest { command_id, enabled, expected_policy_revision };
    let url = format!("/api/v1/admin/worlds/{world_id}");
    let response = WriteRequest::general(&url, Method::PATCH).send_json(&request).await?;
    let payload: ApiResponse<AdminWorldSetEnabledResponse> =
        response.json().await.map_err(AdminWriteFailure::invalid_response)?;
    validate_world_receipt(payload.data.world, &world_id, &request)
}

fn validate_world_receipt(
    accepted: AdminWorldResponse,
    world_id: &str,
    request: &AdminWorldSetEnabledRequest,
) -> Result<AdminBusinessWorld, AdminWriteFailure> {
    let world = decode_world(accepted).map_err(AdminWriteFailure::invalid_response)?;
    let expected_policy_revision = request.expected_policy_revision;
    // 回执必须描述冻结的目标；同版表示状态已相同，下一版表示本次修改。
    if world.id != world_id
        || world.enabled != request.enabled
        || !matches!(
            world.policy_revision,
            revision if revision == expected_policy_revision
                || revision == expected_policy_revision.saturating_add(1)
        )
    {
        return Err(AdminWriteFailure::outcome_unknown("invalid_response"));
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

#[cfg(test)]
mod tests {

    #[test]
    fn world_receipt_matches_the_frozen_world_state_and_revision() {
        use auction_contracts::{
            AdminWorldResponse, AdminWorldSetEnabledRequest, AdminWorldSetEnabledResponse,
            ApiResponse,
        };
        use serde_json::json;
        use uuid::Uuid;

        use super::validate_world_receipt;
        use crate::api::AdminWriteFailure;
        use crate::api::contract_fixtures::admin_fixture;

        let request = AdminWorldSetEnabledRequest {
            command_id: Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap(),
            enabled: false,
            expected_policy_revision: 2,
        };
        let payload: ApiResponse<AdminWorldSetEnabledResponse> =
            serde_json::from_str(admin_fixture!("world-command")).unwrap();
        let mut receipt = serde_json::to_value(payload.data.world).unwrap();
        let validate = |value| {
            let world: AdminWorldResponse = serde_json::from_value(value).unwrap();
            validate_world_receipt(world, "isekai", &request)
        };
        assert_eq!(validate(receipt.clone()).unwrap().policy_revision, 2);
        for (field, wrong) in
            [("id", json!("main")), ("policy_revision", json!(4)), ("enabled", json!(true))]
        {
            let mut invalid = receipt.clone();
            invalid[field] = wrong;
            assert!(
                matches!(
                    validate(invalid),
                    Err(AdminWriteFailure::OutcomeUnknown { code, response_status: None })
                        if code == "invalid_response"
                ),
                "{field}"
            );
        }
        receipt["policy_revision"] = json!(3);
        assert_eq!(validate(receipt).unwrap().policy_revision, 3);
    }

    #[test]
    fn world_contract_fixtures() {
        use super::{
            AdminWorldListResponse, AdminWorldSetEnabledResponse, ApiResponse, decode_world,
            decode_worlds,
        };
        use crate::api::contract_fixtures::{admin_fixture, decode};

        let list: ApiResponse<AdminWorldListResponse> =
            decode(admin_fixture!("worlds"), "/data/worlds/1", "policy_revision");
        let worlds = decode_worlds(list.data.worlds).expect("世界目录");
        assert_eq!(worlds[0].id, "main");
        assert!(worlds[0].enabled);
        assert_eq!(worlds[1].id, "isekai");
        assert!(!worlds[1].enabled);

        let command: ApiResponse<AdminWorldSetEnabledResponse> =
            decode(admin_fixture!("world-command"), "/data/world", "enabled");
        let world = decode_world(command.data.world).expect("世界命令回执");
        assert_eq!((world.id.as_str(), world.enabled, world.policy_revision), ("isekai", false, 2));
    }
}
