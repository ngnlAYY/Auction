//! Admin 录入命令装配；装备密文、目标锁、幂等与审计在 PostgreSQL 事务中提交。

use std::sync::Arc;

use auction_application::AdminIntakeService;
use auction_application::{
    AdminIntakeCommand, AdminIntakePolicy, AdminIntakePort, AdminIntakeResult,
    AdminIntakeSourceReader, ApplicationFuture, Clock, EquipmentKeySealerPort,
};

use super::DatabaseStore;

/// 绑定数据库、装备来源和密钥加密器的 Admin 录入端口。
pub struct DatabaseAdminIntakeCommands<C, S, K> {
    store: Arc<DatabaseStore>,
    source: Arc<S>,
    sealer: Arc<K>,
    clock: Arc<C>,
    policy: AdminIntakePolicy,
}

impl<C, S, K> DatabaseAdminIntakeCommands<C, S, K>
where
    C: Clock + 'static,
    S: AdminIntakeSourceReader + 'static,
    K: EquipmentKeySealerPort + 'static,
{
    /// 建立只向应用层暴露窄端口的 Admin 录入装配器。
    pub fn new(
        store: Arc<DatabaseStore>,
        source: Arc<S>,
        sealer: Arc<K>,
        clock: Arc<C>,
        policy: AdminIntakePolicy,
    ) -> Self {
        Self { store, source, sealer, clock, policy }
    }
}

impl<C, S, K> AdminIntakePort for DatabaseAdminIntakeCommands<C, S, K>
where
    C: Clock + 'static,
    S: AdminIntakeSourceReader + 'static,
    K: EquipmentKeySealerPort + 'static,
{
    fn add_item(&self, command: AdminIntakeCommand) -> ApplicationFuture<'_, AdminIntakeResult> {
        let store = Arc::clone(&self.store);
        let source = Arc::clone(&self.source);
        let sealer = Arc::clone(&self.sealer);
        let clock = Arc::clone(&self.clock);
        let policy = self.policy;
        Box::pin(async move {
            let DatabaseStore::Postgres(store) = store.as_ref();
            let service =
                AdminIntakeService::new(Arc::new(store.clone()), source, sealer, clock, policy);
            service.add_item(command).await
        })
    }
}
