//! Admin runtime 命令账本端口的跨后端分派。

use std::sync::Arc;

use auction_application::admin::{
    AdminRuntimeAuditRecord, AdminRuntimeCommandStore, AdminRuntimeStoredCommand,
};
use auction_application::auction::ApplicationFuture;
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use chrono::{DateTime, Utc};

#[cfg(feature = "postgres")]
use super::{
    mutations::{apply_postgres, insert_pending_postgres, mark_unknown_postgres, reopen_postgres},
    queries::{find_postgres, list_unresolved_postgres},
};
use crate::database::DatabaseStore;

/// 绑定运行时数据库的 Admin runtime 命令账本端口。
pub struct DatabaseAdminRuntimeCommands {
    store: Arc<DatabaseStore>,
}

impl DatabaseAdminRuntimeCommands {
    /// 建立不向应用层暴露具体连接池类型的 runtime 账本端口。
    pub fn new(store: Arc<DatabaseStore>) -> Self {
        Self { store }
    }
}

impl AdminRuntimeCommandStore for DatabaseAdminRuntimeCommands {
    fn find(
        &self,
        command_id: CommandId,
    ) -> ApplicationFuture<'_, Option<AdminRuntimeStoredCommand>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => find_postgres(store, command_id).await,
            }
        })
    }

    fn list_unresolved(&self) -> ApplicationFuture<'_, Vec<AdminRuntimeStoredCommand>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_unresolved_postgres(store).await,
            }
        })
    }

    fn insert_pending(
        &self,
        command: AdminRuntimeStoredCommand,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => insert_pending_postgres(store, command).await,
            }
        })
    }

    fn apply(
        &self,
        command_id: CommandId,
        request_hash: RequestHash,
        audit: AdminRuntimeAuditRecord,
        applied_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    apply_postgres(store, command_id, request_hash, audit, applied_at).await
                }
            }
        })
    }

    fn mark_unknown(
        &self,
        command_id: CommandId,
        request_hash: RequestHash,
        category: String,
        updated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    mark_unknown_postgres(store, command_id, request_hash, category, updated_at)
                        .await
                }
            }
        })
    }

    fn reopen(
        &self,
        command_id: CommandId,
        request_hash: RequestHash,
        updated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminRuntimeStoredCommand> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    reopen_postgres(store, command_id, request_hash, updated_at).await
                }
            }
        })
    }
}
