//! 代理授权与追加式转换事实的数据模型。

use serde::{Deserialize, Serialize};

use crate::{BidAmount, CommandId, LotId, ProxyAuthorizationId, UserId};

/// 代理竞价的调度模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyMode {
    Fast,
    Slow,
}

/// 代理授权的生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyStatus {
    Active,
    FinalFast,
    Exhausted,
    Frozen,
    Closed,
}

impl ProxyStatus {
    pub(super) fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Active, Self::FinalFast)
                | (Self::Active, Self::Exhausted)
                | (Self::Active, Self::Frozen)
                | (Self::Active, Self::Closed)
                | (Self::FinalFast, Self::Exhausted)
                | (Self::FinalFast, Self::Frozen)
                | (Self::FinalFast, Self::Closed)
        )
    }
}

/// 代理授权终态的原因闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyTerminalReason {
    StrictlyDefeated,
    RankedBelow,
    EligibilityLost,
    CompetitionClosed,
}

/// 追加式代理授权转换的分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProxyTransitionKind {
    Created,
    Lowered,
    FinalFast,
    Exhausted,
    Frozen,
    Closed,
}

/// 代理授权当前快照，身份和版本由领域层约束。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyAuthorization {
    pub id: ProxyAuthorizationId,
    pub lot_id: LotId,
    pub user_id: UserId,
    pub creation_command_id: CommandId,
    pub authorization_sequence: u64,
    pub usage_number: u64,
    pub maximum_amount: BidAmount,
    pub mode: ProxyMode,
    pub status: ProxyStatus,
    pub state_version: u64,
}

/// 应用层在事务中追加的代理授权转换事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyTransition {
    pub authorization_id: ProxyAuthorizationId,
    pub command_id: CommandId,
    pub kind: ProxyTransitionKind,
    pub old_maximum: Option<BidAmount>,
    pub new_maximum: BidAmount,
    pub old_mode: Option<ProxyMode>,
    pub new_mode: ProxyMode,
    pub old_status: Option<ProxyStatus>,
    pub new_status: ProxyStatus,
    pub state_version: u64,
    pub reason: Option<ProxyTerminalReason>,
}
