//! Admin 投递详情的只读证据投影。
//!
//! 详情只聚合投递自身、追加式尝试与命令账本，以及维护流程写入的 Sent 证据；
//! 不携带正文、Cookie、登录 key、页面令牌或投递 payload 原文。`credential` 只保留
//! owner 身份版本，`body_digest` 只保留规范摘要，避免管理端成为第二份秘密存储。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::ports::delivery::DeliveryAdminSnapshot;
use crate::ports::delivery::DeliverySendCredentialBinding;

/// 一次冻结 Send 或 Take 尝试的安全投影。
/// `operation` 为 `send` 时 `recipient`/`subject`/`credits`/`cod`/`equipment_ids` 有值；
/// `take` 时 `external_mail_id` 有值。两者都只来自 append-only 尝试行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAdminAttempt {
    /// `send` 或 `take`。
    pub operation: String,
    pub fencing_version: i64,
    pub world_id: String,
    pub sending_at: DateTime<Utc>,
    pub recipient: Option<String>,
    pub subject: Option<String>,
    pub body_digest: Option<[u8; 32]>,
    pub credits: Option<i64>,
    pub cod: Option<i64>,
    pub equipment_ids: Vec<String>,
    pub external_mail_id: Option<String>,
    pub credential: Option<DeliverySendCredentialBinding>,
    /// 该发送尝试已绑定的 Sent MID；未绑定或 Take 尝试为空。
    pub bound_sent_mail_id: Option<String>,
}

/// 维护流程写入投递 `result_metadata` 的可引用 Sent 证据。
///
/// `reference` 是稳定、可被 Admin 命令引用的选择键：`mailbox_sweep:<mid>`、
/// `sent_reference_sighting:<mid>` 或 `compose_discard`。它只用于人工证明的来源核对，
/// 不进入命令摘要，也不改变投递状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAdminSentEvidence {
    pub reference: String,
    /// `mailbox_sweep`、`sent_reference_sighting` 或 `compose_discard`。
    pub kind: String,
    pub external_mail_id: Option<String>,
    pub observed_at: Option<DateTime<Utc>>,
    pub sent_at: Option<DateTime<Utc>>,
    pub recipient: Option<String>,
    pub subject: Option<String>,
    pub body_digest: Option<[u8; 32]>,
    pub collected: Option<bool>,
    pub cod_pending: Option<i64>,
    pub remaining_credits: Option<i64>,
    pub remaining_equipment_count: Option<i64>,
    /// 领取回执摘要；只有人工证明证据携带，自动观察没有回执。
    pub receipt: Option<String>,
}

/// 领取状态：最近一次观测与人工确认责任分别表达，缺失不等于未领取。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAdminCustody {
    pub collected: Option<bool>,
    pub cod_pending: Option<i64>,
    pub remaining_credits: Option<i64>,
    pub receipt: Option<String>,
    pub observed_at: Option<DateTime<Utc>>,
    /// 已有 append-only 人工确认显式接受过领取（`accept_collected_assets`）。
    pub operator_confirmed_collected: bool,
}

/// 已落地的追加式处置事实；来源为 `delivery_admin_commands` 与
/// `delivery_sent_confirmations`，不派生第二套状态机。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAdminDisposition {
    pub command_id: Uuid,
    /// 旧 Admin 命令/人工证明种类，或统一处置表的闭集种类（自动 Sent、装备损失、
    /// 历史自寄、预检退役、操作者确认等）。
    pub kind: String,
    pub occurred_at: DateTime<Utc>,
    pub status: Option<String>,
    pub fencing_version: Option<i64>,
    pub failure_code: Option<String>,
    pub external_reference: Option<String>,
    /// 人工证明的命令发起者；旧 Admin 命令账本自身不保存 actor。
    pub actor: Option<String>,
    pub reason: Option<String>,
}

/// 投递来源的结算义务摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAdminObligation {
    pub obligation_id: Uuid,
    pub auction_id: Uuid,
    pub lot_id: Uuid,
    pub world_id: String,
    pub obligation_kind: String,
    pub business_state: String,
    pub amount: i64,
    pub fulfillment_state: Option<String>,
    pub fulfillment_error_code: Option<String>,
}

/// 投递来源结算义务对应的恢复记录摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAdminRecovery {
    pub recovery_id: Uuid,
    pub stage: String,
    pub resolution: Option<String>,
    pub payment_deadline_at: DateTime<Utc>,
    pub payment_obligation_id: Uuid,
    pub blocking_code: Option<String>,
    pub manual_reconcile_delivery_id: Option<Uuid>,
}

/// 单条投递的完整安全详情。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAdminDetail {
    pub delivery: DeliveryAdminSnapshot,
    /// 冻结尝试按 `sending_at` / fence 倒序，最新在前。
    pub attempts: Vec<DeliveryAdminAttempt>,
    /// 维护证据按观测时间倒序。
    pub evidence: Vec<DeliveryAdminSentEvidence>,
    pub custody: DeliveryAdminCustody,
    /// 处置事实按发生时间倒序。
    pub dispositions: Vec<DeliveryAdminDisposition>,
    pub obligation: Option<DeliveryAdminObligation>,
    pub recovery: Option<DeliveryAdminRecovery>,
}
