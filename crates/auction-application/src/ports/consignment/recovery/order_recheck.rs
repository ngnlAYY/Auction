//! 订单复核工作（方案 A）的 Worker 端口：领取一条未完成的复核工作并列出待核对项，按租约写完成结果。
//!
//! 复核只针对该订单“已起写、无终局证据”的派发尝试（起写无 Sent 绑定）与已起写未证的 Take（起写无结论性
//! 证明），按原 attempt 重读 MoogleMail 核对；查到时经既有证据写口补记（派发 `record_evidence` 的 Sent 绑定、
//! Take 的证明与消费），本端口既不写证据，也不产生任何起写或外写许可（不变量 4）。工作、租约 / fence、
//! 领取快照、完成时刻与结果都持久（不变量 5）。

use auction_domain::SettlementWorld;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::returns::ReturnLeg;
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::errors::ConsignmentFuture;
use crate::ports::delivery::{DeliverySendCredentialBinding, DeliverySendExpectation};

/// 复核结果闭集；与 000137 ⑳ 的结果 CHECK 逐项对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderRecheckOutcome {
    /// 领取快照里的每一项此刻都已有终局证据。
    Bound,
    /// 快照里至少一项仍无终局证据；不重发，留待人工处置。
    StillUnknown,
    /// 领取时没有待核对项。
    NothingToCheck,
}

impl OrderRecheckOutcome {
    /// 全部结果。
    pub const ALL: [Self; 3] = [Self::Bound, Self::StillUnknown, Self::NothingToCheck];

    /// 稳定持久码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bound => "bound",
            Self::StillUnknown => "still_unknown",
            Self::NothingToCheck => "nothing_to_check",
        }
    }

    /// 闭集解析，未知值失败关闭。
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|outcome| outcome.as_str() == value)
    }
}

/// 一次领取的身份：完成时按它核对当前租约。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderRecheckLease {
    /// 复核工作 ID（Admin `recheck` 回执里的 work ID）。
    pub work_id: Uuid,
    /// 本次领取的 token。
    pub claim_token: Uuid,
    /// 本次领取的 fence。
    pub fence: u64,
}

/// 已起写、无 Sent 绑定的派发尝试：按原 attempt 核对所需的冻结事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecheckDispatchAttempt {
    /// 尝试 ID。
    pub attempt_id: Uuid,
    /// 轮 ID。
    pub round_id: Uuid,
    /// 被履行的义务。
    pub obligation_id: Uuid,
    /// 轮次号。
    pub round: u32,
    /// 派发腿持久码（`original`、`carrier`、`cash_payout`、`cash_refund`）。
    pub leg: String,
    /// 投递。
    pub delivery_id: Uuid,
    /// 起写时的 fence。
    pub fence: u64,
    /// 发件 Bot UID。
    pub bot_uid: String,
    /// 世界。
    pub world: SettlementWorld,
    /// 起写时冻结的 Send 身份（收件人、主题、正文摘要、金额、装备与凭据绑定）。
    pub expectation: DeliverySendExpectation,
    /// Sent 核对基线：起写事务冻结的 `sending_at`，Sent 列表只在它之后查找。
    pub sending_at: Timestamp,
    /// 起写时刻。
    pub started_at: Timestamp,
}

/// 已起写、无结论性证明的 Take：按原 attempt 核对所需的事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecheckReturnTake {
    /// 退件接管 ID。
    pub intake_id: Uuid,
    /// 接管的首次决定 ID。
    pub decision_id: Uuid,
    /// 退回腿。
    pub leg: ReturnLeg,
    /// 被接管的原信键 K（RTS 来信）。
    pub mail_key: OriginalMailKey,
    /// Take 工作。
    pub work_id: Uuid,
    /// Take 投递。
    pub delivery_id: Uuid,
    /// 原起写 attempt。
    pub attempt_id: Uuid,
    /// 原起写 fence。
    pub fence: u64,
    /// 原 attempt 的凭据绑定。
    pub credential: DeliverySendCredentialBinding,
    /// 原 attempt 的证据会话。
    pub session_id: Uuid,
    /// 原 attempt 的预期摘要。
    pub expected_digest: [u8; 32],
    /// Take 工作冻结的清单摘要。
    pub manifest_digest: [u8; 32],
    /// 起写时刻。
    pub started_at: Timestamp,
}

/// 一次领取：订单、租约与领取时冻结的待核对项快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderRecheckClaim {
    /// 租约身份。
    pub lease: OrderRecheckLease,
    /// 目标合同。
    pub contract_id: Uuid,
    /// 排队时的合同版本。
    pub contract_version: u64,
    /// 租约到期时刻（数据库时钟）。
    pub lease_until: Timestamp,
    /// 已起写、无 Sent 绑定的派发尝试。
    pub dispatch_attempts: Vec<RecheckDispatchAttempt>,
    /// 已起写、无结论性证明的 Take。
    pub return_takes: Vec<RecheckReturnTake>,
}

/// 订单复核工作的持久领取与完成。
pub trait OrderRecheckWorkStore: Send + Sync {
    /// 以 `SKIP LOCKED` 领取一条未完成、租约空闲或已过期的复核工作，换新 token 并把 fence 加一，同事务冻结
    /// 待核对项快照；没有可领工作时返回 `None`。不改任何订单、投递或证据事实。
    fn claim_due_order_recheck(
        &self,
        worker_id: String,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> ConsignmentFuture<'_, Option<OrderRecheckClaim>>;

    /// 按租约写完成结果并返回它：结果由存储按领取快照与此刻的证据推导，不接受调用方声明。同一租约重复
    /// 完成返回原结果；旧 token、旧 fence 或过租为 `LeaseLost`。
    fn complete_order_recheck(
        &self,
        lease: OrderRecheckLease,
    ) -> ConsignmentFuture<'_, OrderRecheckOutcome>;
}
