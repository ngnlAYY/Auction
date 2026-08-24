//! Admin recovery 报告与确认命令 HTTP 客户端。
//!
//! 报告只暴露后端允许管理端看到的事实。确认命令携带报告中的版本 fence，并由服务端
//! 重新锁定、校验证据和记录幂等结果；浏览器不能把按钮状态当作写入授权。

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

use super::{ApiResponse, fetch_csrf_token};

/// 读取指定原拍卖的 recovery 报告。
pub(crate) async fn fetch_admin_recovery(
    auction_id: String,
) -> Result<AdminRecoveryReport, String> {
    let url = format!("/api/v1/admin/auctions/{auction_id}/recovery-report");
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminRecoveryReport> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

/// 提交带版本 fence 的 recovery 确认命令。
pub(crate) async fn confirm_admin_recovery(
    command_id: Uuid,
    recovery_id: String,
    expected_state_version: i64,
    action: String,
    reason: String,
) -> Result<AdminRecoveryConfirmationResult, String> {
    let csrf_token = fetch_csrf_token().await?;
    let request = Request::post(&format!("/api/v1/admin/recoveries/{recovery_id}/confirm"))
        .header("x-csrf-token", &csrf_token)
        .json(&AdminRecoveryConfirmationRequest {
            command_id,
            expected_state_version,
            action,
            reason,
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminRecoveryConfirmationResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.recovery)
}
