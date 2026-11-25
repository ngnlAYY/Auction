//! 退回处置（resolve_return）的端口命令与回执。
//!
//! 处置只对已提交的精确 Take 消费作出一次：矩阵的决定与决策时冻结的自动重列开关一并落库，
//! 之后改配置不影响已作的决定（J12）。执行者只传消费身份与已冻结的配置值，事实一律由存储在锁内重读。

use auction_domain::CommandId;
use auction_domain::consignment::recovery::ReturnDispositionDecision;
use uuid::Uuid;

use crate::ports::consignment::errors::ConsignmentFuture;
use crate::ports::consignment::scan::{ScanCursor, ScanPage};
use crate::ports::core::RequestHash;

/// 恢复处置系统命令的操作闭集；账本 `operation` 码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecoveryOperation {
    /// 对一次精确 Take 消费作出退回处置。
    ResolveReturn,
    /// 对未付收回处置作出自动重列决定。
    AutoRelist,
    /// 建立或作废付款提醒。
    Remind,
    /// 执行已固定的寄售处罚的账号效果（actor worker，目标 sanction）。
    ApplySanction,
    /// 解禁后买家周期重置（actor system，目标 buyer_cycle）。
    ResetCycle,
    /// 合格退回达阈值时同事务固定待执行处罚（actor worker，目标 buyer_cycle，回执 `sanction_pending`）。
    OpenSanction,
    /// 受限权利通知的派发结果（actor worker，目标 notice_work，回执 `notice_sent/unknown/suppressed`）。
    DeliverRightNotice,
}

impl RecoveryOperation {
    /// 全部操作；账本 `operation` 闭集必须逐项对应（PG 对照测试核对）。
    pub const ALL: [Self; 7] = [
        Self::ResolveReturn,
        Self::AutoRelist,
        Self::Remind,
        Self::ApplySanction,
        Self::ResetCycle,
        Self::OpenSanction,
        Self::DeliverRightNotice,
    ];

    /// 账本 `actor_kind`：解禁重置由系统发起，其余是 Worker 的系统命令。
    pub const fn actor_kind(self) -> &'static str {
        match self {
            Self::ResetCycle => "system",
            Self::ResolveReturn
            | Self::AutoRelist
            | Self::Remind
            | Self::ApplySanction
            | Self::OpenSanction
            | Self::DeliverRightNotice => "worker",
        }
    }

    /// 稳定账本码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ResolveReturn => "resolve_return",
            Self::AutoRelist => "auto_relist",
            Self::Remind => "remind",
            Self::ApplySanction => "apply_sanction",
            Self::ResetCycle => "reset_cycle",
            Self::OpenSanction => "open_sanction",
            Self::DeliverRightNotice => "deliver_right_notice",
        }
    }
}

/// 决策时读一次并冻结的自动重列配置：开关与寄售配置段的规范字节摘要（J12）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoRelistSetting {
    /// `auto_relist_enabled` 的启动时值。
    pub enabled: bool,
    /// 配置摘要；全零表示没有取得配置，写端失败关闭。
    pub config_digest: [u8; 32],
}

/// 退回处置命令：命令 ID 由消费身份派生。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveReturn {
    /// 系统命令 UUIDv5。
    pub command_id: CommandId,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 被处置的精确 Take 消费。
    pub consumption_id: Uuid,
    /// 消费所属的接管；账本目标。
    pub intake_id: Uuid,
    /// 决策时冻结的自动重列配置。
    pub auto_relist: AutoRelistSetting,
}

/// 退回处置回执：`disposition_recorded`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolveReceipt {
    /// 命令 ID。
    pub command_id: CommandId,
    /// 合同。
    pub contract_id: Uuid,
    /// 被处置的消费。
    pub consumption_id: Uuid,
    /// 处置 ID。
    pub disposition_id: Uuid,
    /// 矩阵的决定。
    pub decision: ReturnDispositionDecision,
    /// 是否重放。
    pub replayed: bool,
}

/// 尚无处置的消费。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingResolution {
    /// 消费 ID。
    pub consumption_id: Uuid,
    /// 接管 ID。
    pub intake_id: Uuid,
}

/// 退回处置写端；同命令 ID 同摘要返回原结果，同 ID 异摘要冲突。
pub trait ReturnDispositionStore: Send + Sync {
    /// 锁内重读事实、按矩阵作出处置并落库；证据不足或矛盾时失败关闭，不写处置。
    fn resolve_return(&self, command: ResolveReturn) -> ConsignmentFuture<'_, ResolveReceipt>;

    /// 列出游标之后至多 `limit` 项尚无处置的消费，按 `(consumed_at, 消费 ID)` 升序；读满一页才给出下一页游标。
    fn pending_resolutions(
        &self,
        after: Option<ScanCursor>,
        limit: u32,
    ) -> ConsignmentFuture<'_, ScanPage<PendingResolution>>;
}
