//! Admin recovery 报告与确认命令 HTTP 客户端。
//! 报告只暴露后端允许管理端看到的事实。确认命令携带报告中的版本 fence，并由服务端
//! 重新锁定、校验证据和记录幂等结果；浏览器不能把按钮状态当作写入授权。

use std::collections::HashSet;

pub(crate) use auction_contracts::{
    AdminRecoveryActionResponse as AdminRecoveryAction,
    AdminRecoveryReportResponse as AdminRecoveryReport, AdminRecoveryResponse as AdminRecovery,
};
use auction_contracts::{
    AdminRecoveryConfirmationRequest, AdminRecoveryConfirmationResponse,
    AdminRecoveryConfirmationResultResponse as AdminRecoveryConfirmationResult,
};
use gloo_net::http::Request;
use uuid::Uuid;

use super::{
    AdminApiFailure, ApiResponse, fetch_csrf_token, invalid_request_error, invalid_response_error,
    response_error, response_failure, send_get, send_request, valid_uuid,
};

/// 读取指定原拍卖的 recovery 报告。
pub(crate) async fn fetch_admin_recovery(
    auction_id: String,
) -> Result<AdminRecoveryReport, String> {
    let url = format!("/api/v1/admin/auctions/{auction_id}/recovery-report");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminRecoveryReport> =
        response.json().await.map_err(invalid_response_error)?;
    let report = payload.data;
    validate_recovery_report(&report, &auction_id)
        .then_some(report)
        .ok_or_else(|| invalid_response_error("recovery report wire 无效"))
}

/// 提交带版本 fence 的 recovery 确认命令。
pub(crate) async fn confirm_admin_recovery(
    command_id: Uuid,
    recovery_id: String,
    expected_state_version: i64,
    action: String,
    reason: String,
) -> Result<AdminRecoveryConfirmationResult, AdminApiFailure> {
    let csrf_token = fetch_csrf_token().await.map_err(AdminApiFailure::client)?;
    let url = format!("/api/v1/admin/recoveries/{recovery_id}/confirm");
    let command = AdminRecoveryConfirmationRequest {
        command_id: command_id.to_string(),
        expected_state_version,
        action: action.clone(),
        reason,
    };
    let response = send_request(|abort_signal| {
        Request::post(&url)
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
    let payload: ApiResponse<AdminRecoveryConfirmationResponse> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    let recovery = payload.data.recovery;
    validate_recovery_confirmation(&recovery, &recovery_id, &action, expected_state_version)
        .then_some(recovery)
        .ok_or_else(|| {
            AdminApiFailure::client(invalid_response_error("recovery confirmation 无效"))
        })
}

/// 恢复旧浏览器的严格 report 语义；服务端 mapper 正常不代表 wire 漂移可以被静默接受。
fn validate_recovery_report(report: &AdminRecoveryReport, requested_auction_id: &str) -> bool {
    if !valid_uuid(&report.auction_id) || report.auction_id != requested_auction_id {
        return false;
    }
    let mut recovery_ids = HashSet::with_capacity(report.recoveries.len());
    report.recoveries.iter().all(|recovery| {
        recovery.auction_id == report.auction_id
            && recovery_ids.insert(recovery.id.as_str())
            && validate_recovery(recovery)
    })
}

fn validate_recovery(recovery: &AdminRecovery) -> bool {
    let custody_proof_valid = match (
        recovery.custody_proof_kind.as_deref(),
        recovery.custody_proof_observed_at.as_deref(),
    ) {
        (None, None) => true,
        (Some(kind), Some(observed_at)) => valid_trimmed(kind) && valid_time(observed_at),
        _ => false,
    };
    let relisted_valid = valid_optional_uuid_pair(
        recovery.relisted_auction_id.as_deref(),
        recovery.relisted_lot_id.as_deref(),
    );
    let mut action_ids = HashSet::with_capacity(recovery.actions.len());

    valid_uuid(&recovery.id)
        && valid_uuid(&recovery.auction_id)
        && valid_uuid(&recovery.lot_id)
        && recovery.lot_number > 0
        && matches!(recovery.world_id.as_str(), "main" | "isekai")
        && valid_trimmed(&recovery.equipment_name)
        && valid_trimmed(&recovery.equipment_category)
        && valid_recovery_stage(&recovery.stage)
        && recovery.state_version > 0
        && valid_time(&recovery.payment_deadline_at)
        && recovery.reminder_count >= 0
        && recovery.reminder_limit > 0
        && recovery.reminder_count <= recovery.reminder_limit
        && recovery.resolution.as_deref().is_none_or(valid_recovery_command)
        && custody_proof_valid
        && recovery.blocking_code.as_deref().is_none_or(valid_recovery_error)
        && recovery.manual_reconcile_delivery_id.as_deref().is_none_or(valid_uuid)
        && relisted_valid
        && recovery
            .actions
            .iter()
            .all(|action| action_ids.insert(action.id.as_str()) && validate_recovery_action(action))
        && recovery.allowed_commands.len() <= 1
        && recovery.allowed_commands.iter().all(|command| valid_recovery_command(command))
        && valid_time(&recovery.updated_at)
}

fn validate_recovery_action(action: &AdminRecoveryAction) -> bool {
    let proof_valid = match (action.proof_kind.as_deref(), action.proof_observed_at.as_deref()) {
        (None, None) => true,
        (Some(kind), Some(observed_at)) => valid_trimmed(kind) && valid_time(observed_at),
        _ => false,
    };

    valid_uuid(&action.id)
        && valid_recovery_action_kind(&action.kind)
        && valid_recovery_action_state(&action.state)
        && action.delivery_id.as_deref().is_none_or(valid_uuid)
        && action.failure_code.as_deref().is_none_or(valid_recovery_error)
        && proof_valid
        && valid_time(&action.updated_at)
}

fn validate_recovery_confirmation(
    recovery: &AdminRecoveryConfirmationResult,
    requested_recovery_id: &str,
    requested_action: &str,
    expected_state_version: i64,
) -> bool {
    let relisted = recovery.relisted_auction_id.is_some();
    valid_uuid(&recovery.recovery_id)
        && recovery.recovery_id == requested_recovery_id
        && valid_recovery_command(&recovery.action)
        && recovery.action == requested_action
        && recovery.resolution == recovery.action
        && valid_recovery_stage(&recovery.stage)
        && recovery.state_version > expected_state_version
        && valid_optional_uuid_pair(
            recovery.relisted_auction_id.as_deref(),
            recovery.relisted_lot_id.as_deref(),
        )
        && match recovery.stage.as_str() {
            "relist_pending" => !relisted,
            "refunding" => recovery.action == "equipment_recovered" && relisted,
            "completed" => recovery.action != "equipment_recovered" && relisted,
            _ => false,
        }
}

fn valid_optional_uuid_pair(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => valid_uuid(left) && valid_uuid(right),
        _ => false,
    }
}

/// 浏览器端使用 JS `Date.parse`，精确保留旧 decoder 接受的时间范围。
fn valid_time(value: &str) -> bool {
    !value.is_empty() && js_sys::Date::parse(value).is_finite()
}

fn valid_trimmed(value: &str) -> bool {
    !value.is_empty() && value == value.trim()
}

fn valid_recovery_stage(value: &str) -> bool {
    matches!(
        value,
        "awaiting_payment"
            | "payment_overdue"
            | "delivering_equipment"
            | "reclaiming"
            | "relist_pending"
            | "refunding"
            | "completed"
    )
}

fn valid_recovery_command(value: &str) -> bool {
    matches!(
        value,
        "reclaimed" | "cancelled" | "equipment_recovered" | "returned_after_three_cod_returns"
    )
}

fn valid_recovery_action_kind(value: &str) -> bool {
    matches!(
        value,
        "payment_reminder"
            | "reclaim_equipment_take"
            | "cancel_carrier_take"
            | "recover_equipment_take"
            // 旧后端已有该事实，旧浏览器遗漏；作为明确扩展保留。
            | "buyer_blacklist"
    )
}

fn valid_recovery_action_state(value: &str) -> bool {
    matches!(
        value,
        "pending" | "prepared" | "enqueued" | "satisfied" | "failed" | "manual_reconcile"
    )
}

fn valid_recovery_error(value: &str) -> bool {
    matches!(
        value,
        "invalid_recovery_fact"
            | "recovery_recipient_unavailable"
            | "recovery_delivery_conflict"
            | "recovery_delivery_unavailable"
            | "recovery_delivery_failed"
            | "stale_recovery_claim"
            | "recovery_evidence_not_observed"
            | "recovery_evidence_unavailable"
            | "recovery_evidence_mismatch"
            | "recovery_outcome_unknown"
            | "recovery_relist_unavailable"
    )
}
