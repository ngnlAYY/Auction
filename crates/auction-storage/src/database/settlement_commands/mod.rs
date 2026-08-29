//! 结算命令的数据库装配边界。
//!
//! PostgreSQL 完整档复用应用层 `SettlementService`，确保 Worker 与其他入口共享同一套锁、
//! 幂等账本、结算事实和 outbox 事务；SQLite Lite 明确报告能力缺失。

use std::sync::Arc;

#[cfg(feature = "sqlite")]
use auction_application::ApplicationError;
#[cfg(feature = "postgres")]
use auction_application::SettlementService;
use auction_application::{ApplicationFuture, SettleAuctionCommand, SettlementCommandPort};
use auction_domain::SettlementPlan;
use uuid::Uuid;

use super::DatabaseStore;

#[cfg(feature = "postgres")]
mod legacy;

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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    Err(ApplicationError::CapabilityUnavailable("结算执行仅支持 PostgreSQL 完整档"))
                }
            }
        })
    }
}

impl auction_application::LegacySettlementVerificationPort for DatabaseSettlementCommands {
    fn verify_legacy_settlement(
        &self,
        _auction_id: Uuid,
    ) -> ApplicationFuture<'_, auction_application::LegacySettlementVerification> {
        Box::pin(async move {
            match self.store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => legacy::verify_postgres(store, _auction_id).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "旧版结算账本核验仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}
