//! 退件接管写端口。

use super::commands::{
    AcquireReceipt, AcquireReturn, ConsumeReceipt, ConsumeReturn, StartReturnTake,
};
use crate::ports::consignment::deposit::WriteStartOutcome;
use crate::ports::consignment::errors::ConsignmentFuture;

/// 退件接管写端；每个方法对应一个事务，同命令 ID 同摘要返回原结果，同 ID 异摘要冲突。
pub trait ReturnIntakeStore: Send + Sync {
    /// 按 RTS 的 K 建唯一 intake、Take 工作与终局；锁内按 `decide_acquire_return` 复判，
    /// 隔离与冲突都不落任何接管行。
    fn acquire_return(&self, command: AcquireReturn) -> ConsignmentFuture<'_, AcquireReceipt>;

    /// 提交 Take 起写（与第 4 期 `DispositionStore` 同一起写原语）。只报告本次是否首次提交，
    /// 不构造 POST 许可：许可只能由应用服务据首次提交结果创建并以值移动一次。
    /// 现金腿的 UID opening 阻断随同一事务建立。
    fn start_return_take(
        &self,
        command: StartReturnTake,
    ) -> ConsignmentFuture<'_, WriteStartOutcome>;

    /// 证明 + 消费 + 资金回补同事务；证据不足保留 `reconcile` 并返回 `EvidenceUnproven`。
    fn consume_return(&self, command: ConsumeReturn) -> ConsignmentFuture<'_, ConsumeReceipt>;
}
