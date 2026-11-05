//! delivery 相关 DTO。

use super::*;

/// Admin delivery 安全投影；不包含 payload、lease token 或原始 result metadata。
/// `cod` 只表示经过 owner 校验后从版本化 send payload 提取的金额；其余三个结果字段同样
/// 由 owner 从版本化 metadata 派生，不把 metadata 原文传给浏览器。
/// `deny_unknown_fields` 与 Admin 浏览器端的 fail-closed 解码对齐；wire 冻结后只收紧
/// 反序列化，序列化输出（含 `skip_serializing_if` 省略规则）不变。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryResponse {
    pub id: String,
    pub origin_kind: String,
    pub origin_id: String,
    pub recipient: String,
    pub cod: Option<i64>,
    pub status: String,
    pub lease_expires_at: Option<String>,
    pub fencing_version: i64,
    pub attempt_count: u32,
    pub claimed_at: Option<String>,
    pub sending_at: Option<String>,
    pub completed_at: Option<String>,
    pub external_reference: Option<String>,
    pub failure_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    // 已发布 wire 省略 false，解码时以同一默认值补齐；null 和错误类型仍须拒绝。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub take_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returned_mail_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryListResponse {
    pub deliveries: Vec<AdminDeliveryResponse>,
    pub pagination: AdminDeliveryPaginationResponse,
}

/// Admin delivery 的旧版页码信封。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin 对账判失败可选的固定原因码；历史投递与已提交命令上的既有失败码保持原样。
pub const ADMIN_DELIVERY_RECONCILE_FAILURE_CODES: [&str; 3] =
    ["verified_not_sent", "recipient_invalid", "no_longer_needed"];

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryCommandRequest {
    pub command_id: Uuid,
    pub expected_fencing_version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryCommandResponse {
    pub delivery: AdminDeliveryResponse,
}

/// 投递详情：尝试历史、冻结期望摘要、Sent 观察/目击、领取状态、处置与来源义务。
/// 所有字段都是安全投影：不含投递 payload、正文、Cookie、登录 key 或页面令牌。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryDetailResponse {
    pub delivery: AdminDeliveryResponse,
    /// 冻结 Send/Take 尝试，最新在前。
    pub attempts: Vec<AdminDeliveryAttemptResponse>,
    /// 维护流程写入的可引用 Sent 证据，按观测时间倒序。
    pub evidence: Vec<AdminDeliveryEvidenceResponse>,
    pub custody: AdminDeliveryCustodyResponse,
    /// 追加式处置事实，最新在前。
    pub dispositions: Vec<AdminDeliveryDispositionResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub obligation: Option<AdminDeliveryObligationResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<AdminDeliveryRecoveryResponse>,
}

/// 一次冻结外部动作尝试；`operation=send` 携带冻结内容摘要，`take` 携带目标 MID。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryAttemptResponse {
    pub operation: String,
    pub fencing_version: i64,
    pub world_id: String,
    pub sending_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// 协议规范化正文的 SHA-256（小写 hex），不返回正文。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cod: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub equipment_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_mail_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<AdminDeliveryCredentialResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound_sent_mail_id: Option<String>,
}

/// 原外部动作会话的 owner 身份版本；不含 Cookie、页面令牌或凭据摘要。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryCredentialResponse {
    pub credential_id: String,
    pub credential_version: i64,
    pub runtime_generation: i64,
}

/// 维护流程写入的可引用 Sent 证据；`reference` 是 Admin 命令选择证据时的稳定键。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryEvidenceResponse {
    pub reference: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_mail_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cod_pending: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_credits: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_equipment_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
}

/// 领取状态；`collected` 缺失表示尚未观测，不等于未领取。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryCustodyResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cod_pending: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_credits: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    // 已发布 wire 省略 false，解码时以同一默认值补齐。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub operator_confirmed_collected: bool,
}

/// 追加式处置事实；来源是 Admin 命令账本与人工 Sent 证明。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryDispositionResponse {
    pub command_id: String,
    pub kind: String,
    pub occurred_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fencing_version: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// 投递来源的结算义务摘要。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryObligationResponse {
    pub obligation_id: String,
    pub auction_id: String,
    pub lot_id: String,
    pub world_id: String,
    pub obligation_kind: String,
    pub business_state: String,
    pub amount: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fulfillment_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fulfillment_error_code: Option<String>,
}

/// 投递来源义务对应的恢复记录摘要。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryRecoveryResponse {
    pub recovery_id: String,
    pub stage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    pub payment_deadline_at: String,
    pub payment_obligation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocking_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manual_reconcile_delivery_id: Option<String>,
}

/// 人工 Sent 证明选用的已采集证据；摘要沿用 CLI 的 32 字节数组编码。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliverySentEvidenceRequest {
    pub schema_version: u16,
    pub world_id: String,
    pub external_mail_id: String,
    pub sender_uid: String,
    pub sender_name: String,
    pub recipient: String,
    pub subject: String,
    /// RFC3339 整分钟时间；秒与小数必须为零。
    pub sent_minute: String,
    /// RFC3339 观测时间，不得早于 `sent_minute`。
    pub observed_at: String,
    pub original_body_digest: [u8; 32],
    pub observed_body_digest: [u8; 32],
    pub remaining_credits: i64,
    pub cod_pending: i64,
    #[serde(default)]
    pub equipment_ids: Vec<String>,
    pub collected: bool,
    pub receipt: String,
}

/// 人工 Sent 证明的无副作用预览；不创建命令、资金或审计事实。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliverySentProofPreviewRequest {
    /// 必须命中详情中已写入该投递的维护证据引用。
    pub evidence_reference: String,
    pub evidence: AdminDeliverySentEvidenceRequest,
}

/// 人工 Sent 证明确认；`command_id` 与 CLI 共用同一幂等摘要。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliverySentProofConfirmRequest {
    pub command_id: Uuid,
    pub expected_fencing_version: i64,
    pub reason: String,
    pub evidence_reference: String,
    pub evidence: AdminDeliverySentEvidenceRequest,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub accept_collected_assets: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub accept_attempt_history: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliverySentProofPreviewResponse {
    pub eligible: bool,
    pub blockers: Vec<String>,
    pub delivery_id: String,
    pub expected_fencing_version: i64,
    pub external_reference: String,
    pub requires_collected_asset_acceptance: bool,
    pub requires_attempt_history_acceptance: bool,
    /// 目标是被 Admin 对账为失败的投递；确认即以 Sent 证据追加更正事实。
    pub corrects_admin_failure: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliverySentProofResponse {
    pub command_id: String,
    pub delivery_id: String,
    pub external_reference: String,
    pub replayed: bool,
}
