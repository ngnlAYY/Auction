//! 运行时开关客户端：读取全部开关；写入只开放 W1.9 外部访问暂停。

use super::{
    AdminApiFailure, ApiResponse, fetch_csrf_token, invalid_request_error, invalid_response_error,
    response_error, response_failure, send_get, send_request,
};
use auction_contracts::{
    AdminExternalAccessPauseRequest, AdminExternalAccessPauseResponse, RuntimeSwitchIntentDto,
    RuntimeSwitchListResponse,
};
use chrono::{DateTime, Utc};
use gloo_net::http::Request;
use uuid::Uuid;

const KNOWN_SWITCHES: [&str; 7] = [
    "readiness_audit_failure_mode",
    "readiness_cache_enabled",
    "metrics_sli_snapshot_enabled",
    "mooglemail_poll_isolation_enabled",
    "metrics_reject_forwarded_requests",
    "external_access_paused",
    "disposition_read_mode",
];

pub(crate) async fn fetch_runtime_switches() -> Result<RuntimeSwitchListResponse, String> {
    let response = send_get("/api/v1/admin/runtime-switches").await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<RuntimeSwitchListResponse> =
        response.json().await.map_err(invalid_response_error)?;
    let database_names =
        payload.data.database_intent.iter().map(|item| item.name.as_str()).collect::<Vec<_>>();
    let applied_names =
        payload.data.admin_applied.iter().map(|item| item.name.as_str()).collect::<Vec<_>>();
    if !closed_subset(&database_names)
        || !closed_subset(&applied_names)
        || !applied_names.iter().all(|name| database_names.contains(name))
    {
        return Err("invalid_response".to_owned());
    }
    Ok(payload.data)
}

fn closed_subset(names: &[&str]) -> bool {
    names.iter().all(|name| KNOWN_SWITCHES.contains(name))
        && names.iter().enumerate().all(|(index, name)| !names[..index].contains(name))
}

/// 一次暂停或恢复尝试。结果未知时必须整份原样重试：命令 ID、期望版本、绝对期限与原因
/// 都在首次提交时确定，不能按重试时刻重新计算。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExternalAccessPauseAttempt {
    pub(crate) command_id: Uuid,
    pub(crate) expected_revision: i64,
    pub(crate) paused: bool,
    pub(crate) expires_at: Option<DateTime<Utc>>,
    pub(crate) reason: String,
}

/// 提交 W1.9 外部访问暂停或恢复；只接受与命令一致的已接受事实。
pub(crate) async fn submit_external_access_pause(
    attempt: &ExternalAccessPauseAttempt,
) -> Result<RuntimeSwitchIntentDto, AdminApiFailure> {
    if attempt.command_id.is_nil()
        || attempt.expected_revision < 0
        || attempt.paused != attempt.expires_at.is_some()
        || attempt.reason.trim().is_empty()
    {
        return Err(AdminApiFailure::client("invalid_request"));
    }
    let command = AdminExternalAccessPauseRequest {
        command_id: attempt.command_id.to_string(),
        expected_revision: attempt.expected_revision,
        paused: attempt.paused,
        expires_at: attempt.expires_at,
        reason: attempt.reason.trim().to_owned(),
    };
    let csrf_token = fetch_csrf_token().await.map_err(AdminApiFailure::client)?;
    let response = send_request(|abort_signal| {
        Request::post("/api/v1/admin/runtime-switches/external-access-paused")
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
    let payload: ApiResponse<AdminExternalAccessPauseResponse> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    let accepted = payload.data.accepted;
    let expected_operation = if attempt.paused { "set" } else { "clear" };
    if accepted.name != "external_access_paused"
        || accepted.operation.as_deref() != Some(expected_operation)
        || accepted.revision < 1
    {
        return Err(AdminApiFailure::client(invalid_response_error("external access pause")));
    }
    Ok(accepted)
}

/// 结果是否未知：网络错误、超时、5xx 或服务端明确的 `outcome_unknown` 都只能原样重试。
pub(crate) fn pause_outcome_unknown(failure: &AdminApiFailure) -> bool {
    failure.code == "outcome_unknown" || failure.status.is_none_or(|status| status >= 500)
}
