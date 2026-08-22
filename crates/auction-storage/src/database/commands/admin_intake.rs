//! Admin 装备录入的数据库装配器。
//!
//! 这里不拥有来源读取或密钥加密；它只把运行时选择的 PostgreSQL owner 接到应用服务。
//! SQLite Lite 保留显式能力缺失，避免把缺少 `equipment_secrets` 与完整锁语义的后端伪装成可写。

use std::sync::Arc;

#[cfg(feature = "postgres")]
use auction_application::AdminIntakeService;
#[cfg(feature = "sqlite")]
use auction_application::ApplicationError;
use auction_application::{
    AdminIntakeCommand, AdminIntakePort, AdminIntakeResult, AdminIntakeSourceReader,
    ApplicationFuture, Clock, EquipmentKeySealerPort,
};

use super::DatabaseStore;

/// 绑定数据库、装备来源和密钥加密器的 Admin 录入端口。
pub struct DatabaseAdminIntakeCommands<C, S, K> {
    store: Arc<DatabaseStore>,
    source: Arc<S>,
    sealer: Arc<K>,
    clock: Arc<C>,
}

impl<C, S, K> DatabaseAdminIntakeCommands<C, S, K>
where
    C: Clock + 'static,
    S: AdminIntakeSourceReader + 'static,
    K: EquipmentKeySealerPort + 'static,
{
    /// 建立只向应用层暴露窄端口的 Admin 录入装配器。
    pub fn new(store: Arc<DatabaseStore>, source: Arc<S>, sealer: Arc<K>, clock: Arc<C>) -> Self {
        Self { store, source, sealer, clock }
    }
}

impl<C, S, K> AdminIntakePort for DatabaseAdminIntakeCommands<C, S, K>
where
    C: Clock + 'static,
    S: AdminIntakeSourceReader + 'static,
    K: EquipmentKeySealerPort + 'static,
{
    fn add_item(&self, command: AdminIntakeCommand) -> ApplicationFuture<'_, AdminIntakeResult> {
        #[cfg(feature = "postgres")]
        let store = Arc::clone(&self.store);
        #[cfg(feature = "postgres")]
        let source = Arc::clone(&self.source);
        #[cfg(feature = "postgres")]
        let sealer = Arc::clone(&self.sealer);
        #[cfg(feature = "postgres")]
        let clock = Arc::clone(&self.clock);
        #[cfg(not(feature = "postgres"))]
        let _ = (&command, &self.store, &self.source, &self.sealer, &self.clock);
        Box::pin(async move {
            #[cfg(feature = "postgres")]
            match store.as_ref() {
                DatabaseStore::Postgres(store) => {
                    let service =
                        AdminIntakeService::new(Arc::new(store.clone()), source, sealer, clock);
                    service.add_item(command).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "Admin 装备录入仅支持 PostgreSQL 完整档",
                )),
            }
            #[cfg(not(feature = "postgres"))]
            {
                Err(ApplicationError::CapabilityUnavailable(
                    "Admin 装备录入仅支持 PostgreSQL 完整档",
                ))
            }
        })
    }
}
