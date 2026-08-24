//! Admin recovery 报告与确认命令 HTTP 客户端。
//!
//! 报告只暴露后端允许管理端看到的事实。确认命令携带报告中的版本 fence，并由服务端
//! 重新锁定、校验证据和记录幂等结果；浏览器不能把按钮状态当作写入授权。

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiResponse, fetch_csrf_token};

/// Recovery action 的安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminRecoveryAction {
    pub id: String,
    pub kind: String,
    pub state: String,
    pub delivery_id: Option<String>,
    pub failure_code: Option<String>,
    pub proof_kind: Option<String>,
    pub proof_observed_at: Option<String>,
    pub updated_at: String,
}

/// Recovery case 的安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminRecovery {
    pub id: String,
    pub auction_id: String,
    pub lot_id: String,
    pub lot_number: i32,
    pub world_id: String,
    pub equipment_name: String,
    pub stage: String,
    pub resolution: Option<String>,
    pub state_version: i64,
    pub payment_deadline_at: String,
    pub reminder_count: i32,
    pub reminder_limit: i32,
    pub custody_proof_kind: Option<String>,
    pub custody_proof_observed_at: Option<String>,
    pub blocking_code: Option<String>,
    pub manual_reconcile_delivery_id: Option<String>,
    pub relisted_auction_id: Option<String>,
    pub relisted_lot_id: Option<String>,
    pub actions: Vec<AdminRecoveryAction>,
    pub allowed_commands: Vec<String>,
    pub updated_at: String,
}

/// 一个原拍卖的 recovery 报告。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminRecoveryReport {
    pub auction_id: String,
    pub recoveries: Vec<AdminRecovery>,
}

#[derive(Debug, Clone, Serialize)]
struct AdminRecoveryConfirmationRequest {
    command_id: Uuid,
    expected_state_version: i64,
    action: String,
    reason: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct AdminRecoveryConfirmationData {
    recovery: AdminRecoveryConfirmationResult,
}

/// recovery 确认成功后的安全结果。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminRecoveryConfirmationResult {
    pub recovery_id: String,
    pub action: String,
    pub stage: String,
    pub resolution: String,
    pub state_version: i64,
    pub relisted_auction_id: Option<String>,
    pub relisted_lot_id: Option<String>,
}

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
    let payload: ApiResponse<AdminRecoveryConfirmationData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.recovery)
}
