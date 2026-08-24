//! Admin lot 修改命令的数据库组合器。
//!
//! PostgreSQL 完整档由 `AdminLotUpdateService` 承载 Serializable 事务；SQLite Lite 不具备
//! 当前完整 Admin 生命周期投影，因此必须显式返回能力缺失。

use std::sync::Arc;

#[cfg(feature = "postgres")]
use auction_application::AdminLotUpdateService;
use auction_application::{
    AdminLotSummary, AdminLotUpdateCommand, AdminLotUpdatePort, ApplicationFuture, Clock,
};

use super::super::DatabaseStore;

/// 绑定运行时数据库和时钟的 Admin lot 修改端口。
pub struct DatabaseAdminLotCommands<C> {
    store: Arc<DatabaseStore>,
    clock: Arc<C>,
}

impl<C> DatabaseAdminLotCommands<C>
where
    C: Clock + 'static,
{
    /// 建立不向 HTTP 暴露具体数据库类型的 lot 修改端口。
    pub fn new(store: Arc<DatabaseStore>, clock: Arc<C>) -> Self {
        Self { store, clock }
    }
}

impl<C> AdminLotUpdatePort for DatabaseAdminLotCommands<C>
where
    C: Clock + 'static,
{
    fn update(&self, _command: AdminLotUpdateCommand) -> ApplicationFuture<'_, AdminLotSummary> {
        let _store = Arc::clone(&self.store);
        let _clock = Arc::clone(&self.clock);
        Box::pin(async move {
            match _store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service = AdminLotUpdateService::new(Arc::new(store.clone()), _clock);
                    service.update(_command).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    Err(auction_application::ApplicationError::CapabilityUnavailable(
                        "Admin lot 修改仅支持 PostgreSQL 完整档",
                    ))
                }
            }
        })
    }
}
