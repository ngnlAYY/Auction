//! Admin lot 更新的端口装配；由 AdminLotUpdateService 承载 Serializable 事务。

use std::sync::Arc;

use auction_application::admin::AdminLotUpdateService;
use auction_application::admin::{AdminLotSummary, AdminLotUpdateCommand, AdminLotUpdatePort};
use auction_application::auction::ApplicationFuture;
use auction_application::core::Clock;

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
    pub fn new(store: Arc<DatabaseStore>, clock: Arc<C>) -> Self {
        Self { store, clock }
    }
}

impl<C> AdminLotUpdatePort for DatabaseAdminLotCommands<C>
where
    C: Clock + 'static,
{
    fn update(&self, command: AdminLotUpdateCommand) -> ApplicationFuture<'_, AdminLotSummary> {
        let _store = Arc::clone(&self.store);
        let _clock = Arc::clone(&self.clock);
        Box::pin(async move {
            let store = _store.as_ref();
            let service = AdminLotUpdateService::new(Arc::new(store.clone()), _clock);
            service.update(command).await
        })
    }
}
