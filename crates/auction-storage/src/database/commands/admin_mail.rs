//! 将 Admin 邮件 owner 接入运行时数据库和时钟。

use std::{marker::PhantomData, sync::Arc};

#[cfg(feature = "postgres")]
use auction_application::AdminMailService;
use auction_application::{
    AdminMailCommandPort, AdminMailIntakeDetail, AdminMailIntakeFilter, AdminMailIntakePage,
    AdminMailPollState, AdminMailPollStateRequest, AdminMailPollingCommand, AdminMailReader,
    ApplicationFuture, Clock,
};
use uuid::Uuid;

use super::DatabaseStore;

/// Admin 邮件状态和命令的数据库组合器。
pub struct DatabaseAdminMailCommands<C> {
    store: Arc<DatabaseStore>,
    marker: PhantomData<C>,
    #[cfg(feature = "postgres")]
    clock: Arc<C>,
}

impl<C> DatabaseAdminMailCommands<C>
where
    C: Clock + 'static,
{
    /// 建立不向 HTTP 暴露具体数据库类型的邮件 owner 端口。
    pub fn new(store: Arc<DatabaseStore>, clock: Arc<C>) -> Self {
        #[cfg(not(feature = "postgres"))]
        let _ = clock;
        Self {
            store,
            marker: PhantomData,
            #[cfg(feature = "postgres")]
            clock,
        }
    }
}

impl<C> AdminMailReader for DatabaseAdminMailCommands<C>
where
    C: Clock + 'static,
{
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
        #[cfg(feature = "postgres")]
        let clock = Arc::clone(&self.clock);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    let service = AdminMailService::new(Arc::new(store.clone()), clock);
                    service.set_polling_paused(command).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    let _ = command;
                    Err(auction_application::ApplicationError::CapabilityUnavailable(
                        "Admin 邮件状态修改仅支持 PostgreSQL 完整档",
                    ))
                }
            }
        })
    }
}
