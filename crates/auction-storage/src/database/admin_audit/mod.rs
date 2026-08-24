//! Admin 审计读取适配器的装配与稳定端口。
//!
//! 两个后端都只读追加式 `audit_records`，不向管理端暴露数据库内部行；具体查询和 SQLite
//! 类型转换分别位于后端子模块。

use auction_application::{
    AdminAuditListRequest, AdminAuditPage, AdminAuditPort, ApplicationError, ApplicationFuture,
};

use super::DatabaseStore;

#[cfg(feature = "sqlite")]
mod mapping;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "sqlite")]
mod sqlite;

impl AdminAuditPort for DatabaseStore {
    fn list(&self, request: AdminAuditListRequest) -> ApplicationFuture<'_, AdminAuditPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(ApplicationError::AdminAudit(
                    auction_application::AdminAuditError::InvalidRequest,
                ));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => postgres::list_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => sqlite::list_sqlite(store, request).await,
            }
        })
    }
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(auction_application::RepositoryError::Failed(message.to_owned()))
}
