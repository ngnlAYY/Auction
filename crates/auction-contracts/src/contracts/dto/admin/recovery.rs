//! recovery 相关 DTO。

use super::*;

/// Admin recovery 确认请求；`expected_state_version` 是报告快照中的版本 fence。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRecoveryConfirmationRequest {
    pub command_id: Uuid,
    pub expected_state_version: i64,
    pub action: String,
    #[serde(default)]
    pub reason: String,
}

/// Admin recovery 确认结果的稳定投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRecoveryConfirmationResultResponse {
    pub recovery_id: String,
    pub action: String,
    pub stage: String,
    pub resolution: String,
    pub state_version: i64,
    pub relisted_auction_id: Option<String>,
    pub relisted_lot_id: Option<String>,
}

/// Admin recovery 确认成功响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRecoveryConfirmationResponse {
    pub recovery: AdminRecoveryConfirmationResultResponse,
}

/// Admin recovery action 的安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRecoveryActionResponse {
    pub id: String,
    pub kind: String,
    pub state: String,
    pub delivery_id: Option<String>,
    pub failure_code: Option<String>,
    pub proof_kind: Option<String>,
    pub proof_observed_at: Option<String>,
    pub updated_at: String,
}

/// Admin recovery case 的安全投影，不包含内部 claim/lease 或外部正文。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRecoveryResponse {
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
    pub actions: Vec<AdminRecoveryActionResponse>,
    pub allowed_commands: Vec<String>,
    pub updated_at: String,
}

/// Admin recovery 报告响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRecoveryReportResponse {
    pub auction_id: String,
    pub recoveries: Vec<AdminRecoveryResponse>,
}
