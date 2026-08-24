//! 竞价规划器接收的触发器模型。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{BidAmount, CommandId, ProxyAuthorizationId, ProxyMode, UserId};

/// 纯竞价规划器接受的命令种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompetitionTriggerKind {
    OrdinaryBid,
    CreateProxy,
    LowerProxy,
    SlowDue,
    FinalProxy,
    FreezeProxy,
    CloseProxy,
}

/// 竞价规划器的输入命令。
///
/// 每个变体只携带自己需要的字段，避免用一组可互相矛盾的可选字段表达命令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompetitionTrigger {
    OrdinaryBid {
        command_id: CommandId,
        actor_id: UserId,
        amount: BidAmount,
        bid_ids: Vec<crate::BidId>,
        now: DateTime<Utc>,
    },
    CreateProxy {
        command_id: CommandId,
        actor_id: UserId,
        authorization_id: ProxyAuthorizationId,
        maximum_amount: BidAmount,
        mode: ProxyMode,
        authorization_sequence: u64,
        usage_number: u64,
        bid_ids: Vec<crate::BidId>,
        now: DateTime<Utc>,
    },
    LowerProxy {
        command_id: CommandId,
        actor_id: UserId,
        authorization_id: ProxyAuthorizationId,
        new_maximum: BidAmount,
        now: DateTime<Utc>,
    },
    SlowDue {
        command_id: CommandId,
        authorization_id: ProxyAuthorizationId,
        generation: u64,
        due_at: DateTime<Utc>,
        bid_ids: Vec<crate::BidId>,
        now: DateTime<Utc>,
    },
    FinalProxy {
        command_id: CommandId,
        authorization_id: ProxyAuthorizationId,
        bid_ids: Vec<crate::BidId>,
        now: DateTime<Utc>,
    },
    FreezeProxy {
        command_id: CommandId,
        authorization_id: ProxyAuthorizationId,
        now: DateTime<Utc>,
    },
    CloseProxy {
        command_id: CommandId,
        authorization_id: ProxyAuthorizationId,
        now: DateTime<Utc>,
    },
}

impl CompetitionTrigger {
    /// 返回命令种类，供应用层选择正确的事务和审计编码。
    pub const fn kind(&self) -> CompetitionTriggerKind {
        match self {
            Self::OrdinaryBid { .. } => CompetitionTriggerKind::OrdinaryBid,
            Self::CreateProxy { .. } => CompetitionTriggerKind::CreateProxy,
            Self::LowerProxy { .. } => CompetitionTriggerKind::LowerProxy,
            Self::SlowDue { .. } => CompetitionTriggerKind::SlowDue,
            Self::FinalProxy { .. } => CompetitionTriggerKind::FinalProxy,
            Self::FreezeProxy { .. } => CompetitionTriggerKind::FreezeProxy,
            Self::CloseProxy { .. } => CompetitionTriggerKind::CloseProxy,
        }
    }

    pub(super) fn command_id(&self) -> CommandId {
        match self {
            Self::OrdinaryBid { command_id, .. }
            | Self::CreateProxy { command_id, .. }
            | Self::LowerProxy { command_id, .. }
            | Self::SlowDue { command_id, .. }
            | Self::FinalProxy { command_id, .. }
            | Self::FreezeProxy { command_id, .. }
            | Self::CloseProxy { command_id, .. } => *command_id,
        }
    }

    pub(super) fn now(&self) -> DateTime<Utc> {
        match self {
            Self::OrdinaryBid { now, .. }
            | Self::CreateProxy { now, .. }
            | Self::LowerProxy { now, .. }
            | Self::SlowDue { now, .. }
            | Self::FinalProxy { now, .. }
            | Self::FreezeProxy { now, .. }
            | Self::CloseProxy { now, .. } => *now,
        }
    }

    pub(super) fn bid_ids(&self) -> &[crate::BidId] {
        match self {
            Self::OrdinaryBid { bid_ids, .. }
            | Self::CreateProxy { bid_ids, .. }
            | Self::SlowDue { bid_ids, .. }
            | Self::FinalProxy { bid_ids, .. } => bid_ids,
            Self::LowerProxy { .. } | Self::FreezeProxy { .. } | Self::CloseProxy { .. } => &[],
        }
    }
}
