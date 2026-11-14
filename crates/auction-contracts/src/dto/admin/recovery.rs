//! recovery 相关 DTO。

use serde::{Deserialize, Serialize};

use super::super::shared::{deserialize_null_default, deserialize_required_option};

/// Admin recovery 确认请求；`expected_state_version` 是报告快照中的版本 fence。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRecoveryConfirmationRequest {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub command_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub expected_state_version: i64,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub action: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub reason: String,
}

/// Admin recovery 确认结果的稳定投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRecoveryConfirmationResultResponse {
    /// Rust 内部沿用领域名；旧 v1 wire 固定使用 `id`。
    #[serde(rename = "id")]
    pub recovery_id: String,
    pub action: String,
    pub stage: String,
    pub resolution: String,
    pub state_version: i64,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub relisted_auction_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub relisted_lot_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRecoveryConfirmationResponse {
    pub recovery: AdminRecoveryConfirmationResultResponse,
}

/// Admin recovery action 的安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRecoveryActionResponse {
    pub id: String,
    pub kind: String,
    pub state: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub delivery_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub failure_code: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub proof_kind: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub proof_observed_at: Option<String>,
    pub updated_at: String,
}

/// Admin recovery case 的安全投影，不包含内部 claim/lease 或外部正文。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRecoveryResponse {
    pub id: String,
    pub auction_id: String,
    pub lot_id: String,
    pub lot_number: i32,
    pub world_id: String,
    pub equipment_name: String,
    pub equipment_category: String,
    pub stage: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub resolution: Option<String>,
    pub state_version: i64,
    pub payment_deadline_at: String,
    pub reminder_count: i32,
    pub reminder_limit: i32,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub custody_proof_kind: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub custody_proof_observed_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub blocking_code: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub manual_reconcile_delivery_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub relisted_auction_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub relisted_lot_id: Option<String>,
    pub actions: Vec<AdminRecoveryActionResponse>,
    pub allowed_commands: Vec<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRecoveryReportResponse {
    pub auction_id: String,
    pub recoveries: Vec<AdminRecoveryResponse>,
}
