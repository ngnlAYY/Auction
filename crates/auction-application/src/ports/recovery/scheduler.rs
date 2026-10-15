use chrono::{DateTime, Duration, Utc};

use auction_domain::{CommandId, SettlementId};
use uuid::Uuid;

use super::{ApplicationFuture, RequestHash};

/// Worker 推进 recovery case 后返回的安全摘要。
///
/// 该摘要不包含 claim token、外部邮件正文或 delivery payload；它只用于记录本轮推进结果，
/// 真正的 recovery action 仍由 recovery owner 的后续端口领取和执行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryCaseAdvance {
    pub recovery_id: SettlementId,
    pub stage: String,
    pub reminder_count: i32,
    pub state_version: i64,
    pub completed: bool,
    pub advanced_at: DateTime<Utc>,
}

/// Worker 推进 recovery relist/refund resolution 后返回的安全摘要。
///
/// 重挂本身和退款闭合都必须由 recovery owner 在数据库事务内完成；Worker 只记录结果，不能
/// 根据这份摘要自行修改 recovery、lot 或 obligation。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryResolutionAdvance {
    pub recovery_id: SettlementId,
    pub stage: String,
    pub resolution: Option<String>,
    pub relisted_auction_id: Option<Uuid>,
    pub relisted_lot_id: Option<Uuid>,
    pub state_version: i64,
    pub completed: bool,
    pub advanced_at: DateTime<Utc>,
}

/// Worker 记录一封结算退回邮件后返回的安全摘要。
///
/// recovery 只会把已由 mail processor 解析并持久化的证据与 obligation 重新比对；摘要不携带
/// 邮件正文、装备 key 或外部凭据。证据不匹配时仍会记录退回邮件，但不会创建可执行 action。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReturnAdvance {
    pub recovery_id: SettlementId,
    pub return_mail_id: Uuid,
    pub stage: String,
    pub blocking_code: Option<String>,
    pub action_kinds: Vec<String>,
    pub state_version: i64,
    pub recorded_at: DateTime<Utc>,
}

/// Worker 领取的一条 recovery action。
///
/// `recipient` 可能为空，表示 Users owner 没有可用的 MM 收件人名称；这种确定性事实必须进入
/// manual reconcile，而不能由 Worker 猜测收件人或静默丢弃 action。`recipient_eligible` 是
/// Users owner 在同一读取投影中给出的 active 状态；prepare/activation 前必须为 true，已经
/// enqueued 的 action 观察结果时不再用它回滚外部事实。`recipient_state_version` 用于校验
/// 这份 Users 安全投影确实来自有效的用户事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryActionClaim {
    pub action_id: SettlementId,
    pub recovery_id: SettlementId,
    pub kind: String,
    pub world_id: String,
    pub buyer_user_id: Uuid,
    pub recipient: Option<String>,
    pub recipient_eligible: bool,
    pub recipient_state_version: i64,
    pub payment_reference: String,
    pub source_external_mail_id: Option<String>,
    pub expected_credits: i64,
    pub expected_cod: i64,
    pub expected_attachment_ids: Vec<String>,
    pub execution_state: String,
    pub delivery_id: Option<Uuid>,
    pub prepare_command_id: Option<CommandId>,
    pub prepare_request_hash: Option<RequestHash>,
    pub activate_command_id: Option<CommandId>,
    pub activate_request_hash: Option<RequestHash>,
    pub state_version: i64,
    pub claim_token: Uuid,
    pub lease_until: DateTime<Utc>,
    pub fencing_version: i64,
    pub retry_count: u32,
    pub worker_id: String,
    pub claimed_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// recovery delivery 观察后的持久化结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryActionOutcome {
    /// delivery 尚未完成，action 已被释放并等待下一次观察。
    Retry,
    /// delivery 已成功，action 已写入可审计证明。
    Satisfied { proof_kind: String, proof_reference: String },
    /// delivery 确定失败，action 已终止。
    Failed { failure_code: String },
    /// 外部结果未知或事实冲突，action 已转人工复核。
    ManualReconcile { failure_code: String },
}

/// recovery case 的持久化推进端口。
///
/// PostgreSQL 完整档在同一 Serializable 事务中锁定 due case、检查结算履约事实、推进阶段并
/// 创建 payment reminder action，不以内存定时器替代持久事实。
pub trait RecoveryWorkStore: Send + Sync {
    /// 记录一封尚未消费的结算退回邮件，并在同一事务中创建后续 recovery action。
    fn record_due_return(
        &self,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<RecoveryReturnAdvance>>;

    /// 推进一个当前可处理的 case；没有 due 工作时返回 `None`。
    fn advance_due_case(
        &self,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<RecoveryCaseAdvance>>;

    /// 领取一条 payment reminder 或后续 recovery action；返回值必须来自数据库 fence。
    fn claim_due_action(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<RecoveryActionClaim>>;

    /// 将已准备的 delivery 与 pending action 原子关联。
    fn link_action_preparation(
        &self,
        claim: RecoveryActionClaim,
        delivery_id: Uuid,
        command_id: CommandId,
        request_hash: RequestHash,
        linked_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, RecoveryActionClaim>;

    /// 以 activation identity 将 prepared action 推进为 enqueued。
    fn mark_action_enqueued(
        &self,
        claim: RecoveryActionClaim,
        activate_command_id: CommandId,
        activate_request_hash: RequestHash,
        activated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, RecoveryActionClaim>;

    /// 记录 Users owner 已成功完成的非邮件 action，并以当前 claim fence 写入 proof。
    fn mark_action_satisfied(
        &self,
        claim: RecoveryActionClaim,
        proof_kind: String,
        proof_reference: String,
        proof_digest: [u8; 32],
        observed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;

    /// 读取 delivery 结果并在同一 fencing 条件下推进 action。
    fn observe_action(
        &self,
        claim: RecoveryActionClaim,
        observed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, RecoveryActionOutcome>;

    /// 外部 owner 暂时不可用时释放 action 并设置持久化退避。
    fn release_action(
        &self,
        claim: RecoveryActionClaim,
        retry_at: DateTime<Utc>,
        failure_code: String,
    ) -> ApplicationFuture<'_, ()>;

    /// 记录确定性的 owner/事实缺失，阻止 action 被无限重试。
    fn mark_action_manual_reconcile(
        &self,
        claim: RecoveryActionClaim,
        failure_code: String,
    ) -> ApplicationFuture<'_, ()>;
}

/// recovery resolution 的持久化调度端口。
///
/// `relist_pending` 的目标拍卖可能暂时不存在，因此重挂事务必须支持持久化退避；`refunding`
/// 只能依据 buyer refund fulfillment 的已提交完成事实闭合。两项操作都不能由内存计时器代替。
pub trait RecoveryResolutionStore: Send + Sync {
    /// 推进一个到期的 guarded relist；没有 due 工作时返回 `None`。
    fn advance_due_relist(
        &self,
        now: DateTime<Utc>,
        retry_after: Duration,
    ) -> ApplicationFuture<'_, Option<RecoveryResolutionAdvance>>;

    /// 在 buyer refund fulfillment 已满足后闭合一个 refunding case。
    fn complete_due_refund(
        &self,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<RecoveryResolutionAdvance>>;
}
