use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use auction_domain::{
    AuctionId, CommandId, EquipmentId, LotId, SettlementId, SettlementObligationKind,
    SettlementWorld, SubmissionId,
};

use super::{ApplicationFuture, RequestHash};

/// 结算履约当前允许由 Worker 推进的持久阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementFulfillmentState {
    Pending,
    Prepared,
    /// 外部发送已执行，等待 Sent（以及必要时 Balance）证明。
    Enqueued,
}

/// 结算义务交给 delivery owner 时使用的操作闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementFulfillmentOperation {
    Send,
    Return,
}

impl SettlementFulfillmentOperation {
    /// 返回迁移表使用的稳定操作名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Return => "return",
        }
    }
}

/// Worker 从数据库领取的一条结算履约执行事实。
///
/// 这里携带的是不含 secret 的义务快照。用户收件人已经在同一数据库快照中解析为
/// `game_player_id`；不能在领取阶段把密钥或 Cookie 复制到任务对象。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementFulfillmentClaim {
    pub obligation_id: SettlementId,
    pub auction_id: AuctionId,
    pub lot_id: LotId,
    pub origin_command_id: CommandId,
    pub world: SettlementWorld,
    pub kind: SettlementObligationKind,
    pub state: SettlementFulfillmentState,
    pub operation: SettlementFulfillmentOperation,
    pub recipient: String,
    pub amount: i64,
    pub carrier_credit: u32,
    pub equipment_id: EquipmentId,
    /// 远端 MoogleMail/装备协议使用的外部装备 ID；不含装备 key。
    pub external_equipment_id: String,
    /// 结算通知正文使用的装备名称；只允许来自同一数据库快照。
    pub equipment_name: String,
    pub submission_id: Option<SubmissionId>,
    /// 投稿来源闭集；邮件来源才能执行远端原邮件退回。
    pub submission_source_kind: Option<String>,
    /// 旧邮件的远端 ID，仅在邮件来源时使用；不保存正文或 Cookie。
    pub submission_external_mail_id: Option<String>,
    pub prerequisite_id: Option<SettlementId>,
    pub delivery_id: Option<Uuid>,
    /// delivery 成功结果中的外部引用；只有经过传输层证明的发送才可用于 exact Sent 查询。
    pub external_reference: Option<String>,
    /// 外部引用是否由发送成功页明确证明，而不是本地 fallback。
    pub external_reference_proven: bool,
    pub prepare_command_id: Option<CommandId>,
    pub prepare_request_hash: Option<RequestHash>,
    pub claim_token: Uuid,
    pub claim_fencing: i64,
    pub state_version: u64,
    pub worker_id: String,
    pub claimed_at: DateTime<Utc>,
    pub lease_until: DateTime<Utc>,
    pub attempt_count: u32,
}

/// Sent 页面返回的、可进入履约证明的安全快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementFulfillmentSentEvidence {
    pub external_mail_id: String,
    pub world: SettlementWorld,
    pub recipient: String,
    pub subject: String,
    pub remaining_credits: i64,
    pub cod_pending: i64,
    pub remaining_equipment_ids: Vec<String>,
    pub collected: bool,
    pub observed_at: DateTime<Utc>,
}

/// Balance 页面返回的、可用于发送前资金检查的安全快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementFulfillmentBalanceEvidence {
    pub world: SettlementWorld,
    pub credits: i64,
    pub observed_at: DateTime<Utc>,
}

/// Worker 与 MoogleMail owner 之间的外部证明读取窄端口。
pub trait SettlementFulfillmentEvidenceReader: Send + Sync {
    fn read_sent(
        &self,
        world: SettlementWorld,
        external_mail_id: String,
    ) -> ApplicationFuture<'_, SettlementFulfillmentSentEvidence>;

    fn read_balance(
        &self,
        world: SettlementWorld,
    ) -> ApplicationFuture<'_, SettlementFulfillmentBalanceEvidence>;
}

/// 结算履约满足条件后写入的不可变证明摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementFulfillmentProof {
    pub kind: String,
    pub reference: String,
    pub digest: [u8; 32],
    pub observed_at: DateTime<Utc>,
}

/// 履约回执读取后的持久状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementFulfillmentReconcileStatus {
    /// 已写入不可变外部证明并将义务推进到 satisfied。
    Satisfied,
    /// 已安全停止自动推进，等待管理员依据外部事实人工复核。
    ManualReconcile,
}

/// 履约回执读取成功后返回的持久身份和状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementFulfillmentReconcileResult {
    pub obligation_id: SettlementId,
    pub delivery_id: Uuid,
    pub status: SettlementFulfillmentReconcileStatus,
}

/// Delivery owner 写入 prepared outbox 所需的稳定身份和 canonical payload。
#[derive(Debug, Clone, PartialEq)]
pub struct SettlementDeliveryPreparation {
    pub delivery_id: Uuid,
    pub prepare_command_id: CommandId,
    pub request_hash: RequestHash,
    pub recipient: String,
    pub payload: Value,
}

/// 履约准备成功后返回的持久身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementFulfillmentResult {
    pub obligation_id: SettlementId,
    pub delivery_id: Uuid,
    pub prepare_command_id: CommandId,
}

/// 激活 prepared delivery 后返回的持久身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementFulfillmentActivationResult {
    pub obligation_id: SettlementId,
    pub delivery_id: Uuid,
    pub activate_command_id: CommandId,
}

/// 结算履约调度的窄存储端口。
///
/// PostgreSQL 完整档负责领取、租约 fencing、prepared delivery 的幂等落表和失败退避。SQLite
/// Lite 没有这组完整履约表时必须显式返回能力缺失，不能退化成内存队列。
pub trait SettlementFulfillmentStore: Send + Sync {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<SettlementFulfillmentClaim>>;

    fn prepare_delivery(
        &self,
        claim: SettlementFulfillmentClaim,
        preparation: SettlementDeliveryPreparation,
        prepared_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, SettlementFulfillmentResult>;

    fn activate_delivery(
        &self,
        claim: SettlementFulfillmentClaim,
        activate_command_id: CommandId,
        activate_request_hash: RequestHash,
        activated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, SettlementFulfillmentActivationResult>;

    fn satisfy(
        &self,
        claim: SettlementFulfillmentClaim,
        proof: SettlementFulfillmentProof,
    ) -> ApplicationFuture<'_, SettlementFulfillmentReconcileResult>;

    fn manual_reconcile(
        &self,
        claim: SettlementFulfillmentClaim,
        failure_code: String,
    ) -> ApplicationFuture<'_, ()>;

    fn fail(
        &self,
        claim: SettlementFulfillmentClaim,
        retry_at: DateTime<Utc>,
        failure_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, ()>;
}

/// 结算履约用例端口；Worker 只负责调度，不自己拼接或写入 delivery payload。
pub trait SettlementFulfillmentCommandPort: Send + Sync {
    fn prepare(
        &self,
        claim: SettlementFulfillmentClaim,
        prepared_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, SettlementFulfillmentResult>;

    fn activate(
        &self,
        claim: SettlementFulfillmentClaim,
        activated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, SettlementFulfillmentActivationResult>;

    fn reconcile(
        &self,
        claim: SettlementFulfillmentClaim,
    ) -> ApplicationFuture<'_, SettlementFulfillmentReconcileResult>;
}
