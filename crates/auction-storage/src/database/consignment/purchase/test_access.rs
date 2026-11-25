//! 购买事务的 PG 测试窄入口；只在 `test-db` 构建。

use auction_application::consignment::{ConsignmentError, PurchaseCommand, PurchaseOutcome};

pub use crate::database::consignment::refusal::test_access::TestAfterLock;

use super::PurchaseRepository;

impl PurchaseRepository {
    /// 带钩子的购买：`after_locks` 在取全部会话锁之后、开快照之前运行，用于在买家与卖家锁下交换提交顺序。
    pub async fn test_purchase_hooked(
        &self,
        command: PurchaseCommand,
        after_locks: Option<&TestAfterLock>,
    ) -> Result<PurchaseOutcome, ConsignmentError> {
        self.run_purchase(command, after_locks).await
    }
}
