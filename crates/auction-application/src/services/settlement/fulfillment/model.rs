//! 结算履约跨 owner 传递的无 secret 模型。

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

/// 交给 delivery owner 的 canonical intent，不是 MoogleMail 的最终表单 payload。
#[derive(Debug, Serialize)]
pub(super) struct SettlementDeliveryIntent {
    pub(super) schema_version: u8,
    pub(super) operation: &'static str,
    pub(super) world: &'static str,
    pub(super) obligation_id: Uuid,
    pub(super) auction_id: Uuid,
    pub(super) lot_id: Uuid,
    pub(super) origin_command_id: Uuid,
    pub(super) kind: &'static str,
    pub(super) recipient: String,
    pub(super) template: &'static str,
    pub(super) item_name: String,
    pub(super) external_equipment_id: String,
    pub(super) amount: i64,
    pub(super) carrier_credit: u32,
    pub(super) credits: i64,
    pub(super) cod: i64,
    pub(super) equipment_id: Uuid,
    pub(super) submission_id: Option<Uuid>,
    pub(super) submission_source_kind: Option<String>,
    pub(super) source_external_mail_id: Option<String>,
}

/// 激活 prepared delivery 时计算的稳定身份和阶段 fence。
#[derive(Debug, Serialize)]
pub(super) struct SettlementDeliveryActivation {
    pub(super) schema_version: u8,
    pub(super) delivery_id: Uuid,
    pub(super) obligation_id: Uuid,
    pub(super) prepare_request_hash: [u8; 32],
    pub(super) expected_status: &'static str,
}

/// Sent/Balance 证据组成的稳定 proof 摘要载荷。
#[derive(Debug, Serialize)]
pub(super) struct SettlementProofDigest<'a> {
    pub(super) schema_version: u8,
    pub(super) obligation_id: Uuid,
    pub(super) delivery_id: Uuid,
    pub(super) external_mail_id: &'a str,
    pub(super) world: &'a str,
    pub(super) recipient: &'a str,
    pub(super) subject: &'a str,
    pub(super) remaining_credits: i64,
    pub(super) cod_pending: i64,
    pub(super) remaining_equipment_ids: &'a [String],
    pub(super) collected: bool,
    pub(super) sent_observed_at: DateTime<Utc>,
    pub(super) balance_credits: Option<i64>,
    pub(super) balance_observed_at: Option<DateTime<Utc>>,
}
