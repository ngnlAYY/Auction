//! 竞价命令的冻结请求、目标与提交结果分类。

use std::rc::Rc;

use auction_time::Timestamp;
use auction_web_shared::t;
use uuid::Uuid;

use super::pending::{PendingBidCommand, PendingProxyCommand};
use crate::api::{CommandError, create_proxy_authorization, lower_proxy_authorization, place_bid};

/// 命令发出时目录所属的场次。
#[derive(Clone, PartialEq)]
pub(in crate::surface) struct CommandAuction {
    pub auction_id: String,
    pub world: String,
}

/// 冻结的请求目标与用于恢复入口的拍品标签。
#[derive(Clone, Debug, PartialEq)]
pub(in crate::surface) struct CommandTarget {
    pub world: String,
    pub auction_id: String,
    pub lot_id: String,
    pub lot_code: String,
    pub lot_name: String,
}

/// 两阶段确认交给身份宿主的完整负载；原样重试不从当前表单或目录重建字段。
#[derive(Clone, Debug, PartialEq)]
pub(in crate::surface) enum CommandPayload {
    Bid(PendingBidCommand),
    Proxy(PendingProxyCommand),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::surface) enum CommandKind {
    Bid,
    CreateProxy,
    LowerProxy,
}

impl CommandKind {
    pub(super) fn label(self) -> String {
        match self {
            Self::Bid => t!("bid-tab-direct"),
            Self::CreateProxy => t!("command-create-proxy"),
            Self::LowerProxy => t!("bid-type-proxy-lower"),
        }
    }
}

impl CommandPayload {
    pub(super) fn user_id(&self) -> Uuid {
        match self {
            Self::Bid(PendingBidCommand { user_id, .. })
            | Self::Proxy(
                PendingProxyCommand::Create { user_id, .. }
                | PendingProxyCommand::Lower { user_id, .. },
            ) => *user_id,
        }
    }

    pub(super) fn command_id(&self) -> Uuid {
        match self {
            Self::Bid(PendingBidCommand { command_id, .. })
            | Self::Proxy(
                PendingProxyCommand::Create { command_id, .. }
                | PendingProxyCommand::Lower { command_id, .. },
            ) => *command_id,
        }
    }

    pub(super) fn amount(&self) -> i64 {
        match self {
            Self::Bid(PendingBidCommand { amount, .. })
            | Self::Proxy(
                PendingProxyCommand::Create { amount, .. }
                | PendingProxyCommand::Lower { amount, .. },
            ) => *amount,
        }
    }

    pub(super) fn kind(&self) -> CommandKind {
        match self {
            Self::Bid(_) => CommandKind::Bid,
            Self::Proxy(PendingProxyCommand::Create { .. }) => CommandKind::CreateProxy,
            Self::Proxy(PendingProxyCommand::Lower { .. }) => CommandKind::LowerProxy,
        }
    }

    pub(super) async fn send(&self, target: &CommandTarget) -> Result<(), CommandError> {
        let lot_id = target.lot_id.clone();
        // 两次请求使用同一组冻结字段与同一个 API 序列化入口，保留 UUID、授权路径和正文。
        match self {
            Self::Bid(PendingBidCommand { user_id, amount, command_id }) => {
                place_bid(*user_id, lot_id, *amount, *command_id).await.map(|_| ())
            }
            Self::Proxy(PendingProxyCommand::Create { user_id, amount, mode, command_id }) => {
                create_proxy_authorization(*user_id, lot_id, *amount, mode.clone(), *command_id)
                    .await
                    .map(|_| ())
            }
            Self::Proxy(PendingProxyCommand::Lower {
                user_id,
                amount,
                authorization_id,
                command_id,
            }) => lower_proxy_authorization(
                *user_id,
                lot_id,
                authorization_id.clone(),
                *amount,
                *command_id,
            )
            .await
            .map(|_| ()),
        }
    }
}

/// 首次提交在途，或继续保留一个尚未确认的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::surface) enum CommandStatus {
    Submitting,
    Unknown { retrying: bool, error: CommandError },
}

impl CommandStatus {
    pub(in crate::surface) fn busy(&self) -> bool {
        !matches!(self, Self::Unknown { retrying: false, .. })
    }

    pub(in crate::surface) fn unknown(&self) -> bool {
        matches!(self, Self::Unknown { .. })
    }

    pub(in crate::surface) fn entry_message(&self) -> String {
        if self.unknown() { t!("command-lot-unresolved") } else { t!("bid-working") }
    }
}

/// 一个未结命令及其首次发出时刻；只有同身份的同一负载可以重放。
#[derive(Clone, PartialEq)]
pub(in crate::surface) struct SentCommand {
    pub target: Rc<CommandTarget>,
    pub(super) payload: Rc<CommandPayload>,
    pub(super) sent_at: Timestamp,
    pub status: CommandStatus,
}

/// 已知结果的回显数据。价格、领先关系与授权状态由读取资源另行更新。
#[derive(Clone, PartialEq)]
pub(in crate::surface) struct CommandReceipt {
    pub(super) command_id: Uuid,
    pub target: Rc<CommandTarget>,
    pub kind: CommandKind,
    pub completion: CommandCompletion,
}

#[derive(Clone, PartialEq)]
pub(in crate::surface) enum CommandCompletion {
    Accepted,
    Rejected(CommandError),
}

impl CommandReceipt {
    pub(in crate::surface) fn accepted(&self) -> bool {
        matches!(self.completion, CommandCompletion::Accepted)
    }
}

pub(super) fn command_conflict(error: &CommandError) -> bool {
    (error.status, error.code.as_str()) == (409, "command_conflict")
}

/// 业务拒绝出现在命令账本查询之后，可以了结；入口拒绝可能早于回放，仍保留原始未知结果。
pub(super) fn outcome_unknown(error: &CommandError, retry: bool) -> bool {
    if error.not_sent() {
        // 本地发送前的失败不能覆盖此前已经发出的未知结果。
        return retry;
    }
    if command_conflict(error) {
        // 摘要不匹配与事务唯一约束冲突共用此码，无法从它确认此前的结果。
        return retry;
    }
    error.outcome_uncertain()
        || !(400..500).contains(&error.status)
        || (retry
            && !matches!(
                (error.status, error.code.as_str()),
                (403, "bidder_ineligible")
                    | (
                        409,
                        "bid_too_low"
                            | "already_leading"
                            | "active_proxy_exists"
                            | "auction_not_open"
                            | "proxy_authorization_not_active"
                            | "proxy_maximum_not_lower"
                            | "proxy_maximum_below_binding"
                            | "proxy_reentry_maximum_not_higher"
                            | "proxy_disabled"
                            | "proxy_outbid_limit"
                            | "proxy_slow_final"
                            | "invalid_policy"
                    )
            ))
}
