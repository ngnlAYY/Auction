//! Admin 审计端口装配；只读 PostgreSQL 的追加式 audit_records，不暴露数据库行。

use auction_application::admin::{AdminAuditListRequest, AdminAuditPage, AdminAuditPort};
use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;

use crate::database::DatabaseStore;

mod postgres;

impl AdminAuditPort for DatabaseStore {
    fn list(&self, request: AdminAuditListRequest) -> ApplicationFuture<'_, AdminAuditPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(ApplicationError::AdminAudit(
                    auction_application::admin::AdminAuditError::InvalidRequest,
                ));
            }
            postgres::list_postgres(self, request).await
        })
    }
}
