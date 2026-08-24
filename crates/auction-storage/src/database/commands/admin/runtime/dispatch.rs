//! Admin runtime 命令账本端口的跨后端分派。

use std::sync::Arc;

use auction_application::{
    AdminRuntimeAuditRecord, AdminRuntimeCommandStore, AdminRuntimeStoredCommand,
    ApplicationFuture, RequestHash,
};
use auction_domain::CommandId;
use chrono::{DateTime, Utc};

use super::super::DatabaseStore;
#[cfg(feature = "sqlite")]
use super::errors::lite_unavailable;
#[cfg(feature = "postgres")]
use super::{
    mutations::{apply_postgres, insert_pending_postgres, mark_unknown_postgres, reopen_postgres},
    queries::{find_postgres, list_unresolved_postgres},
};

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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    let _ = command_id;
                    Err(lite_unavailable())
                }
            }
        })
    }

    fn list_unresolved(&self) -> ApplicationFuture<'_, Vec<AdminRuntimeStoredCommand>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            match store.as_ref() {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_unresolved_postgres(store).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(lite_unavailable()),
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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    let _ = command;
                    Err(lite_unavailable())
                }
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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    let _ = (command_id, request_hash, audit, applied_at);
                    Err(lite_unavailable())
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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    let _ = (command_id, request_hash, category, updated_at);
                    Err(lite_unavailable())
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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => {
                    let _ = (command_id, request_hash, updated_at);
                    Err(lite_unavailable())
                }
            }
        })
    }
}
