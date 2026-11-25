//! v2 本人寄售命令回执的只读端口（D9 §6.1、§6.3）。
//!
//! 只按原玩家 actor 与命令 ID 读取账本中已提交的回执；非本人与不存在同为 `None`。
//! 未读到行只代表当次未见，不证明未知提交已经失败（不变量 1）。

use auction_domain::{CommandId, UserId};

use crate::ports::consignment::deposit::{ReservationReceipt, WithdrawDepositReceipt};
use crate::ports::consignment::errors::{ConsignmentFuture, DepositBusinessRejection};
use crate::ports::consignment::purchase::PurchaseOutcome;
use crate::ports::consignment::recovery::RelistReceipt;

/// 收件族命令入账的首次决定：成功回执或确定拒绝。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedDepositDecision<T> {
    /// 首次成功回执。
    Accepted(T),
    /// 首次确定拒绝，只带安全码。
    Rejected(DepositBusinessRejection),
}

/// 一条本人 v2 写命令的已入账结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedCommandV2 {
    /// 买家购买；拒绝沿用购买自身的拒绝闭集。
    Purchase(PurchaseOutcome),
    /// 卖家重列。
    Relist(OwnedDepositDecision<RelistReceipt>),
    /// 卖家 v2 撤回。
    Withdraw(OwnedDepositDecision<WithdrawDepositReceipt>),
    /// 卖家签发寄件预约。
    IssueReservation(OwnedDepositDecision<ReservationReceipt>),
}

/// 仅从已守卫的 Public 会话取得 actor，再读取其 v2 写命令的原回执。
pub trait OwnedCommandReadV2: Send + Sync {
    /// 不变量 1、7：只返回 actor 为该玩家的 v2 命令；非本人、v1 命令、系统命令与不存在均为 `None`。
    fn owned_command_v2(
        &self,
        actor_user_id: UserId,
        command_id: CommandId,
    ) -> ConsignmentFuture<'_, Option<OwnedCommandV2>>;
}
