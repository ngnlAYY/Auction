//! Admin 邮件查询与命令的 PostgreSQL 端口装配。

use auction_application::admin::{
    AdminMailDashboardSnapshot, AdminMailError, AdminMailIntakeDetail, AdminMailIntakeFilter,
    AdminMailIntakePage, AdminMailPollState, AdminMailPollStateRequest, AdminMailReader,
};
use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use uuid::Uuid;

use crate::database::DatabaseStore;

mod mapping;
mod reads;
mod transaction;

impl AdminMailReader for DatabaseStore {
    fn dashboard_snapshot(&self) -> ApplicationFuture<'_, AdminMailDashboardSnapshot> {
        Box::pin(async move { reads::dashboard_snapshot_postgres(self).await })
    }

    fn poll_state(
        &self,
        request: AdminMailPollStateRequest,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        Box::pin(async move {
            request.validate().map_err(ApplicationError::from)?;
            reads::get_poll_state_postgres(self, request).await
        })
    }

    fn list_intakes(
        &self,
        request: AdminMailIntakeFilter,
    ) -> ApplicationFuture<'_, AdminMailIntakePage> {
        Box::pin(async move {
            request.validate().map_err(ApplicationError::from)?;
            reads::list_intakes_postgres(self, request).await
        })
    }

    fn intake_detail(
        &self,
        intake_id: Uuid,
    ) -> ApplicationFuture<'_, Option<AdminMailIntakeDetail>> {
        Box::pin(async move {
            if intake_id.is_nil() {
                return Err(ApplicationError::AdminMail(AdminMailError::InvalidRequest));
            }
            reads::get_intake_postgres(self, intake_id).await
        })
    }
}
