//! Admin lot 更新的端口装配；由 AdminLotUpdateService 承载 Serializable 事务。

use std::sync::Arc;

#[cfg(feature = "postgres")]
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
            }
        })
    }
}
