//! Admin 订单 recheck 与 resolve 的命令、回执与写端口。
//!
//! resolve 的动作是 D9 §6.2 的闭集；其中退款类三个动作共用同一个 6B 写端，补寄只能开新轮。回执不含
//! 买家、收件人、金额明细或装备 key。账本操作名 recheck / resolve 不在拍卖 `DepositOperation` 闭集内
//! （该闭集是集成人的串行点），这里沿派发四个操作的先例自带一个只含这两项的闭集。

use auction_domain::CommandId;
use uuid::Uuid;

use crate::ports::consignment::errors::ConsignmentFuture;
use crate::ports::core::RequestHash;

/// 订单级 Admin 命令的账本操作名闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderAdminOperation {
    /// 排入订单复核工作。
    Recheck,
    /// 记录处置或排队补寄。
    Resolve,
}

impl OrderAdminOperation {
    /// 全部操作；与 000137 追加段的 admin 操作与回执分支逐项对应。
    pub const ALL: [Self; 2] = [Self::Recheck, Self::Resolve];

    /// 稳定 wire 与账本码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recheck => "recheck",
            Self::Resolve => "resolve",
        }
    }
}

/// resolve 的动作闭集（D9 §6.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderResolveAction {
    /// 取消未付购买：证据是“关闭原买方交割”的退回处置，写端是 7A-2 的 `cancel_unpaid`。
    CancelUnpaid,
    /// 原件退回后全额退款。
    RefundReturned,
    /// 实物灭失后全额退款。
    RefundLost,
    /// 换季清空后全额退款。
    RefundSeason,
    /// 保留原受益用途：证据是现金腿退回处置，与退款转换同属 6B 写端。
    RetainEntitlement,
    /// 补寄原件：只开新轮，上一轮必须已明确封闭。
    RedispatchOriginal,
}

impl OrderResolveAction {
    /// 全部动作；顺序是 D9 的声明顺序。
    pub const ALL: [Self; 6] = [
        Self::CancelUnpaid,
        Self::RefundReturned,
        Self::RefundLost,
        Self::RefundSeason,
        Self::RetainEntitlement,
        Self::RedispatchOriginal,
    ];

    /// 稳定 wire 与账本码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CancelUnpaid => "cancel_unpaid",
            Self::RefundReturned => "refund_returned",
            Self::RefundLost => "refund_lost",
            Self::RefundSeason => "refund_season",
            Self::RetainEntitlement => "retain_entitlement",
            Self::RedispatchOriginal => "redispatch_original",
        }
    }

    /// 闭集解析，未知值失败关闭。
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.as_str() == value)
    }

    /// 退款类动作共用同一个 6B 写端。
    pub const fn is_refund(self) -> bool {
        matches!(self, Self::RefundReturned | Self::RefundLost | Self::RefundSeason)
    }
}

/// Admin 排入订单复核。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecheckOrderCommand {
    /// 已认证 Admin 稳定引用。
    pub admin_actor: String,
    /// Admin 会话 UUIDv4。
    pub command_id: CommandId,
    /// 目标合同。
    pub contract_id: Uuid,
    /// 已提交合同版本。
    pub expected_version: u64,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 仅审计关联。
    pub request_id: Option<String>,
}

/// Admin 对订单作处置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveOrderCommand {
    /// 已认证 Admin 稳定引用。
    pub admin_actor: String,
    /// Admin 会话 UUIDv4。
    pub command_id: CommandId,
    /// 目标合同。
    pub contract_id: Uuid,
    /// 已提交合同版本。
    pub expected_version: u64,
    /// 处置动作。
    pub action: OrderResolveAction,
    /// 证据 UUID；含义随动作而定（退件消费、损失证据、季末证明或封闭上一轮的退件接管）。
    pub evidence_id: Uuid,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 仅审计关联。
    pub request_id: Option<String>,
}

/// 复核已排队：`recheck_queued`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecheckOrderReceipt {
    /// 命令 ID。
    pub command_id: CommandId,
    /// 合同 UUID。
    pub contract_id: Uuid,
    /// 回执时的合同版本。
    pub contract_version: u64,
    /// 复核工作 UUID。
    pub work_id: Uuid,
}

/// resolve 的成功回执：处置已记录，或补寄已排队。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderResolveReceipt {
    /// `resolution_recorded`。
    ResolutionRecorded {
        /// 命令 ID。
        command_id: CommandId,
        /// 合同 UUID。
        contract_id: Uuid,
        /// 回执时的合同版本。
        contract_version: u64,
        /// 处置动作（非补寄）。
        action: OrderResolveAction,
        /// 处置记录 UUID。
        resolution_id: Uuid,
    },
    /// `redispatch_queued`：动作固定为补寄。
    RedispatchQueued {
        /// 命令 ID。
        command_id: CommandId,
        /// 合同 UUID。
        contract_id: Uuid,
        /// 回执时的合同版本。
        contract_version: u64,
        /// 将要开的新轮的轮 ID。
        dispatch_id: Uuid,
    },
}

/// Admin 订单处置的写端口。同命令 ID 同请求返回原回执，异请求为 `command_id_conflict`。
pub trait OrderAdminStore: Send + Sync {
    /// 排入订单级复核工作；不改任何订单状态。
    fn recheck_order(
        &self,
        command: RecheckOrderCommand,
    ) -> ConsignmentFuture<'_, RecheckOrderReceipt>;

    /// 记录处置；退款类动作调用同一个 6B 写端，存在相反付款时拒绝并不留资金痕迹。
    fn resolve_order(
        &self,
        command: ResolveOrderCommand,
    ) -> ConsignmentFuture<'_, OrderResolveReceipt>;
}

#[cfg(test)]
mod tests {
    use super::{OrderAdminOperation, OrderResolveAction};

    /// D9 §6.2 的动作闭集逐字钉住；任何增删都必须同时改迁移里的动作 CHECK。
    #[test]
    fn resolve_actions_are_the_d9_closed_set() {
        let codes = OrderResolveAction::ALL.map(OrderResolveAction::as_str);
        assert_eq!(
            codes,
            [
                "cancel_unpaid",
                "refund_returned",
                "refund_lost",
                "refund_season",
                "retain_entitlement",
                "redispatch_original",
            ]
        );
        assert_eq!(OrderResolveAction::parse("refund_lost"), Some(OrderResolveAction::RefundLost));
        assert_eq!(OrderResolveAction::parse("REFUND_LOST"), None);
        assert_eq!(OrderResolveAction::parse("refund"), None);
        assert_eq!(OrderResolveAction::parse(""), None);
        let refunds = OrderResolveAction::ALL.into_iter().filter(|action| action.is_refund());
        assert_eq!(refunds.count(), 3);
    }

    #[test]
    fn order_operations_match_the_ledger_codes() {
        assert_eq!(
            OrderAdminOperation::ALL.map(OrderAdminOperation::as_str),
            ["recheck", "resolve"]
        );
    }
}
