//! 购买与 v2 改价的存储端口。

use super::command::{PurchaseCommand, RepriceV2Command};
use super::receipt::{PurchaseOutcome, RepriceV2Outcome};
use crate::ports::consignment::errors::ConsignmentFuture;

/// 购买写端口。
pub trait PurchaseStore: Send + Sync {
    /// 单个 Serializable 事务：命令仲裁 → 锁内事实 → `decide_purchase` → 一次写 Sale、义务图、
    /// 首轮、挂单、position、事件与回执。
    ///
    /// 保护不变量 1、2、3、6：同 ID 同请求回原回执，同 ID 异请求冲突；任一步失败整笔回滚；
    /// 确定业务拒绝留安全回执；D0 由锁后时刻加冻结策略派生，不接受外部传入。
    fn purchase(&self, command: PurchaseCommand) -> ConsignmentFuture<'_, PurchaseOutcome>;

    /// 只读重放：按命令 ID 查账本，不取锁、不写任何行、不看开关与资格。
    ///
    /// 保护不变量 1：同 ID 同摘要返回原结果（成交回执或确定拒绝），同 ID 异摘要为
    /// `CommandIdConflict`；账本里没有该命令为 `None`，由调用方再按开关决定能否受理新命令。
    fn replay_purchase(
        &self,
        command: PurchaseCommand,
    ) -> ConsignmentFuture<'_, Option<PurchaseOutcome>>;
}

/// v2 改价写端口。
pub trait RepriceV2Store: Send + Sync {
    /// 单个 Serializable 事务内按 `decide_reprice_available` 判定并写挂单价、版本、事件与回执；
    /// 已有 Sale 或代管承诺的挂单一律 `equipment_committed`，不改价（不变量 2、6、10）。
    fn reprice(&self, command: RepriceV2Command) -> ConsignmentFuture<'_, RepriceV2Outcome>;
}
