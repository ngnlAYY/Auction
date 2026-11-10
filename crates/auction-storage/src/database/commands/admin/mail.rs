//! 将 Admin 邮件 owner 接入运行时数据库和时钟。

use std::{marker::PhantomData, sync::Arc};

use auction_application::admin::AdminMailService;
use auction_application::admin::{
    AdminMailCommandPort, AdminMailDashboardSnapshot, AdminMailIntakeDetail, AdminMailIntakeFilter,
    AdminMailIntakePage, AdminMailPollState, AdminMailPollStateRequest,
    AdminMailPollTriggerCommand, AdminMailPollingCommand, AdminMailReader,
};
use auction_application::auction::ApplicationFuture;
use auction_application::core::Clock;
use uuid::Uuid;

use super::super::DatabaseStore;

/// Admin 邮件状态和命令的数据库组合器。
pub struct DatabaseAdminMailCommands<C> {
    store: Arc<DatabaseStore>,
    marker: PhantomData<C>,
    clock: Arc<C>,
}

impl<C> DatabaseAdminMailCommands<C>
where
    C: Clock + 'static,
{
    /// 建立不向 HTTP 暴露具体数据库类型的邮件 owner 端口。
    pub fn new(store: Arc<DatabaseStore>, clock: Arc<C>) -> Self {
        Self { store, marker: PhantomData, clock }
    }
}

impl<C> AdminMailReader for DatabaseAdminMailCommands<C>
where
    C: Clock + 'static,
{
    fn dashboard_snapshot(&self) -> ApplicationFuture<'_, AdminMailDashboardSnapshot> {
        let store = Arc::clone(&self.store);
        Box::pin(async move { AdminMailReader::dashboard_snapshot(store.as_ref()).await })
    }

    fn poll_state(
        &self,
        request: AdminMailPollStateRequest,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        let store = Arc::clone(&self.store);
        Box::pin(async move { AdminMailReader::poll_state(store.as_ref(), request).await })
    }

    fn list_intakes(
        &self,
        request: AdminMailIntakeFilter,
    ) -> ApplicationFuture<'_, AdminMailIntakePage> {
        let store = Arc::clone(&self.store);
        Box::pin(async move { AdminMailReader::list_intakes(store.as_ref(), request).await })
    }

    fn intake_detail(
        &self,
        intake_id: Uuid,
    ) -> ApplicationFuture<'_, Option<AdminMailIntakeDetail>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move { AdminMailReader::intake_detail(store.as_ref(), intake_id).await })
    }
}

impl<C> AdminMailCommandPort for DatabaseAdminMailCommands<C>
where
    C: Clock + 'static,
{
    fn set_polling_paused(
        &self,
        command: AdminMailPollingCommand,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        let store = Arc::clone(&self.store);
        let clock = Arc::clone(&self.clock);
        Box::pin(async move {
            let store = store.as_ref();
            let service = AdminMailService::new(Arc::new(store.clone()), clock);
            service.set_polling_paused(command).await
        })
    }

    fn trigger_poll(
        &self,
        command: AdminMailPollTriggerCommand,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        let store = Arc::clone(&self.store);
        let clock = Arc::clone(&self.clock);
        Box::pin(async move {
            let store = store.as_ref();
            let service = AdminMailService::new(Arc::new(store.clone()), clock);
            service.trigger_poll(command).await
        })
    }
}
