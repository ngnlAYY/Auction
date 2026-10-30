//! 结算命令装配；幂等账本、结算事实和 outbox 在同一 PostgreSQL 事务提交。

use std::sync::Arc;

#[cfg(feature = "postgres")]
use auction_application::SettlementService;
use auction_application::{ApplicationFuture, SettleAuctionCommand, SettlementCommandPort};
use auction_domain::SettlementPlan;

use super::DatabaseStore;

/// 绑定运行时数据库的结算命令端口。
pub struct DatabaseSettlementCommands {
    store: Arc<DatabaseStore>,
}

impl DatabaseSettlementCommands {
    /// 建立不向 Worker 暴露具体数据库事务类型的结算端口。
    pub fn new(store: Arc<DatabaseStore>) -> Self {
        Self { store }
    }
}

impl SettlementCommandPort for DatabaseSettlementCommands {
    fn settle(&self, _command: SettleAuctionCommand) -> ApplicationFuture<'_, SettlementPlan> {
        Box::pin(async move {
            match self.store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    SettlementService::new(Arc::new(store.clone())).settle(_command).await
                }
            }
        })
    }
}
