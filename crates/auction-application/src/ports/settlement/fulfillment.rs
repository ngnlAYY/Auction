use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use uuid::Uuid;

use auction_domain::{
    AuctionId, CommandId, EquipmentId, EquipmentKeyEnvelope, LotId, SettlementId, SettlementLine,
    SettlementMode, SettlementObligationKind, SettlementWorld, SubmissionId,
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
    /// 仅为读取并隔离旧执行记录保留；不得用于新的结算履约。
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

/// fulfillment owner 允许观察的 delivery 持久状态闭集。
/// delivery 只拥有传输状态；fulfillment 必须读取这份安全快照后，才能决定延期、失败、
/// 人工复核或写入外部证明，不能由 delivery 事务跨 owner 直接改写义务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementFulfillmentDeliveryStatus {
    Prepared,
    Pending,
    Claimed,
    Sending,
    Succeeded,
    Failed,
    ManualReconcile,
}

/// fulfillment reconcile 使用的 delivery 安全快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementFulfillmentDeliverySnapshot {
    pub id: Uuid,
    pub status: SettlementFulfillmentDeliveryStatus,
    pub prepare_command_id: CommandId,
    pub prepare_request_hash: RequestHash,
    pub activate_command_id: Option<CommandId>,
    pub activate_request_hash: Option<RequestHash>,
    pub origin_kind: String,
    pub origin_id: String,
    pub recipient: String,
    pub payload: Value,
    pub external_reference: Option<String>,
    /// Send 由远端 Sent 身份证明；旧 Return 标记仅用于历史快照，不能证明装备返还。
    pub external_reference_proven: bool,
    pub failure_code: Option<String>,
}

/// 返回旧版 MoogleMail fulfillment 发送与 Sent 证据共同使用的 canonical 主题。
/// reference 固定为稳定 obligation ID；流拍与撤拍均逐件发送原装备。
pub fn canonical_settlement_fulfillment_subject(
    kind: SettlementObligationKind,
    obligation_id: SettlementId,
) -> Option<String> {
    let subject = match kind {
        SettlementObligationKind::WinnerEquipmentCod
        | SettlementObligationKind::WinnerEquipmentHandoff
        | SettlementObligationKind::UnsoldReturn
        | SettlementObligationKind::WithdrawnReturn => "Auction equipment delivery",
        SettlementObligationKind::WinnerPaymentCarrierCod => "Auction payment request",
        SettlementObligationKind::SellerPayout => "Auction seller payout",
        SettlementObligationKind::BuyerRefund => "Auction buyer refund",
    };
    Some(format!("{subject} [{obligation_id}]"))
}

/// Worker 从数据库领取的一条结算履约执行事实。
/// 这里携带的是不含 secret 的义务快照。领取时的当前玩家/MM 名称只用于构造初始上下文；
/// pending/prepared/enqueued 仍按各自旧阶段规则通过 users owner 复核当前身份。不能在领取阶段
/// 把密钥或 Cookie 复制到任务对象。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementFulfillmentClaim {
    pub obligation_id: SettlementId,
    pub auction_id: AuctionId,
    pub lot_id: LotId,
    pub origin_command_id: CommandId,
    /// delivery/MoogleMail 外部操作使用的派发世界。
    pub world: SettlementWorld,
    pub kind: SettlementObligationKind,
    pub state: SettlementFulfillmentState,
    pub operation: SettlementFulfillmentOperation,
    /// Users owner 收件人的稳定内部 ID；卖家和退回收件人使用冻结的 MM 名称时为空。
    pub recipient_user_id: Option<Uuid>,
    pub recipient: String,
    pub amount: i64,
    pub carrier_credit: u32,
    pub equipment_id: EquipmentId,
    /// custody 读取使用的装备来源世界；异世界退款与 `world` 不同。
    pub equipment_world: SettlementWorld,
    /// 远端 MoogleMail/装备协议使用的外部装备 ID；不含装备 key。
    pub external_equipment_id: String,
    /// 结算通知正文使用的装备名称；只允许来自同一数据库快照。
    pub equipment_name: String,
    /// 竞得装备邮件使用的冻结结算明细；不从当前竞价或现行奖惩配置重算。
    pub settlement_line: Option<SettlementLine>,
    /// 与明细同源的冻结模式；v2 账单必须明确名义比例与实分奖池优惠的区别。
    pub settlement_mode: Option<SettlementMode>,
    /// recovery case 在结算事务内冻结的付款期限；只用于正文呈现与证据轮询聚焦，
    /// 逾期判定仍由 recovery owner 负责。
    /// 该值缺失只可能是没有 recovery 行的历史义务，因此不进入必填不变量。
    pub payment_deadline_at: Option<DateTime<Utc>>,
    pub submission_id: Option<SubmissionId>,
    /// 投稿来源闭集；流拍与撤拍均根据来源授权逐件发送原装备。
    pub submission_source_kind: Option<String>,
    /// 旧邮件的远端 ID，仅在邮件来源时使用；不保存正文或 Cookie。
    pub submission_external_mail_id: Option<String>,
    pub prerequisite_id: Option<SettlementId>,
    pub delivery_id: Option<Uuid>,
    pub prepare_command_id: Option<CommandId>,
    pub prepare_request_hash: Option<RequestHash>,
    pub activate_command_id: Option<CommandId>,
    pub activate_request_hash: Option<RequestHash>,
    pub delivery_snapshot: Option<SettlementFulfillmentDeliverySnapshot>,
    pub claim_token: Uuid,
    pub claim_fencing: i64,
    pub state_version: u64,
    pub worker_id: String,
    pub claimed_at: DateTime<Utc>,
    pub lease_until: DateTime<Utc>,
    /// 已持久化释放次数；领取本身不增加该值。
    pub retry_count: u32,
}

/// users owner 返回的当前履约收件人投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementFulfillmentRecipientSnapshot {
    pub user_id: Uuid,
    pub game_identity: String,
    pub eligible: bool,
    pub state_version: u64,
}

/// 当前装备 key 的加密信封及其 owner 身份。
/// 该结构只在 Worker 组合根交给 custody 适配器，不进入 claim、日志或外部 DTO。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementFulfillmentEquipmentSecret {
    pub equipment_id: EquipmentId,
    pub external_equipment_id: String,
    pub envelope: EquipmentKeyEnvelope,
}

/// 只读诊断所需的已冻结结算发送目标；不包含 secret、claim 或 lease 身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementFulfillmentSentLookupTarget {
    pub obligation_id: SettlementId,
    pub world: SettlementWorld,
    pub recipient: String,
    pub external_equipment_id: String,
    pub credits: i64,
    pub cod: i64,
    pub sending_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// users owner 的当前收件人读取端口。
pub trait SettlementFulfillmentRecipientReader: Send + Sync {
    fn read_recipient(
        &self,
        user_id: Uuid,
    ) -> ApplicationFuture<'_, SettlementFulfillmentRecipientSnapshot>;
}

/// storage owner 向 Worker custody 适配器提供的加密装备 key 读取端口。
pub trait SettlementFulfillmentEquipmentSecretReader: Send + Sync {
    fn read_equipment_secret(
        &self,
        equipment_id: EquipmentId,
    ) -> ApplicationFuture<'_, SettlementFulfillmentEquipmentSecret>;
}

/// Worker 与唯一邮件 owner 之间的正向库存证明端口。
/// 返回值必须持有认证会话；调用方将 guard 保留到 prepare/activate 事务确定结束。
pub trait SettlementFulfillmentCustodyReader: Send + Sync {
    fn read_custody(
        &self,
        request: crate::InternalEvidenceRequest,
    ) -> crate::InternalEvidenceFuture<'_, crate::PreparedSettlementInternalEvidence>;
}

/// 读取冻结 delivery 目标供离线 Sent 核对；实现不得领取、续租或写回状态。
pub trait SettlementFulfillmentSentLookupReader: Send + Sync {
    fn read_sent_lookup_target(
        &self,
        obligation_id: Uuid,
    ) -> ApplicationFuture<'_, SettlementFulfillmentSentLookupTarget>;
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

/// 履约读取当前 bot 游戏身份的窄端口；只接受同一 active Cookie/world 的远端证明。
/// 保留 Option 签名兼容已有适配器；真实读取成功返回 Some，失败必须返回错误。
/// 未配置预期名称仍需远端联合核验，不能以 None 跳过识别或降级为配置名称。
/// 此端口不授权满足义务：内部留存还须独立验证真实托管、资金、依赖和事务凭据版本。
pub trait SettlementFulfillmentBotIdentityReader: Send + Sync {
    fn read_verified_player_identity(
        &self,
        world: SettlementWorld,
    ) -> ApplicationFuture<'_, Option<crate::VerifiedMailPlayerIdentity>>;
}

/// Worker 与 MoogleMail owner 之间的外部证明读取窄端口。
pub trait SettlementFulfillmentEvidenceReader: Send + Sync {
    fn read_sent(
        &self,
        world: SettlementWorld,
        external_mail_id: String,
    ) -> ApplicationFuture<'_, SettlementFulfillmentSentEvidence>;

    /// 使用应用层冻结的观测时间读取余额，避免外部适配器把未来时间写入健康投影。
    fn read_balance(
        &self,
        world: SettlementWorld,
        observed_at: DateTime<Utc>,
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
    /// delivery 或外部证据仍在推进，已持久化下一次扫描时间。
    Deferred,
    /// 已写入不可变外部证明并将义务推进到 satisfied。
    Satisfied,
    /// delivery 已确定失败，fulfillment 已进入不可自动重领的 failed 终态。
    Failed,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementFulfillmentResult {
    pub obligation_id: SettlementId,
    pub delivery_id: Uuid,
    pub prepare_command_id: CommandId,
    /// 保留原 claim 的 token/fencing，并携带已推进到 prepared 的下一阶段工作。
    pub next_claim: SettlementFulfillmentClaim,
}

/// 激活 prepared delivery 后返回的持久身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementFulfillmentActivationResult {
    pub obligation_id: SettlementId,
    pub delivery_id: Uuid,
    pub activate_command_id: CommandId,
    /// 保留原 claim 的 token/fencing，并携带已推进到 enqueued 的下一阶段工作。
    pub next_claim: SettlementFulfillmentClaim,
}

/// dependency 暂不可用或事实冲突属于已经持久化的业务分支，不应再由 Worker 当作普通错误
/// 以另一个错误码重复释放同一 claim。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettlementFulfillmentPhaseResult<T> {
    Advanced(T),
    Deferred { obligation_id: SettlementId, failure_code: &'static str },
    ManualReconcile { obligation_id: SettlementId, failure_code: &'static str },
}

/// 结算履约调度的窄存储端口。
/// PostgreSQL 负责领取、租约 fencing、prepared delivery 幂等落表和失败退避，不退化成内存队列。
pub trait SettlementFulfillmentStore: Send + Sync {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<SettlementFulfillmentClaim>>;

    fn prepare_delivery<'a>(
        &'a self,
        claim: SettlementFulfillmentClaim,
        preparation: SettlementDeliveryPreparation,
        prepared_at: DateTime<Utc>,
        custody: Option<(&'a crate::SettlementInternalEvidenceSnapshot, std::time::Instant)>,
    ) -> ApplicationFuture<'a, SettlementFulfillmentResult>;

    fn activate_delivery<'a>(
        &'a self,
        claim: SettlementFulfillmentClaim,
        activate_command_id: CommandId,
        activate_request_hash: RequestHash,
        activated_at: DateTime<Utc>,
        // 保留参数以兼容既有适配器；当前激活继续持有 claim，调用方须传 None。
        // COD/货款的下次 MM 追踪时间在 reconcile 释放 claim 的同一事务内写入。
        next_attempt_at: Option<DateTime<Utc>>,
        custody: Option<(&'a crate::SettlementInternalEvidenceSnapshot, std::time::Instant)>,
    ) -> ApplicationFuture<'a, SettlementFulfillmentActivationResult>;

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

    fn mark_failed(
        &self,
        claim: SettlementFulfillmentClaim,
        failure_code: String,
    ) -> ApplicationFuture<'_, ()>;

    fn fail(
        &self,
        claim: SettlementFulfillmentClaim,
        retry_delay: Duration,
        failure_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, ()>;
}

/// 结算履约用例端口；Worker 只负责调度，不自己拼接或写入 delivery payload。
pub trait SettlementFulfillmentCommandPort: Send + Sync {
    /// 准备下一阶段：外部履约返回 delivery 准备结果，已由内部证明满足的义务返回无 delivery 结果。
    fn prepare_next(
        &self,
        claim: SettlementFulfillmentClaim,
        prepared_at: DateTime<Utc>,
        retry_delay: Duration,
    ) -> ApplicationFuture<'_, crate::SettlementFulfillmentPreparationOutcome>;

    fn activate(
        &self,
        claim: SettlementFulfillmentClaim,
        activated_at: DateTime<Utc>,
        retry_delay: Duration,
    ) -> ApplicationFuture<
        '_,
        SettlementFulfillmentPhaseResult<SettlementFulfillmentActivationResult>,
    >;

    /// 读取发送回执并收口证据。`retry_at` 是调度器按配置基础间隔给出的最早重试点：
    /// 证据未观察的延期会按履约年龄指数退避（上限 10 分钟，付款截止前后回到基础间隔），
    /// 依赖与身份错误仍按该基础间隔重试；COD/货款等待继续沿用共享追踪窗口的对齐时刻。
    fn reconcile(
        &self,
        claim: SettlementFulfillmentClaim,
        observed_at: DateTime<Utc>,
        retry_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, SettlementFulfillmentReconcileResult>;
}
