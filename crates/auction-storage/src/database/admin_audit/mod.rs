//! Admin 审计端口装配；只读 PostgreSQL 的追加式 audit_records，不暴露数据库行。

use auction_application::{
    AdminAuditListRequest, AdminAuditPage, AdminAuditPort, ApplicationError, ApplicationFuture,
};

use super::DatabaseStore;
use crate::database::storage_error;

#[cfg(feature = "postgres")]
mod postgres;

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
            }
        })
    }
}
