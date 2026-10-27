//! Admin 邮件查询与命令的 PostgreSQL 端口装配。

use auction_application::{
    AdminMailDashboardSnapshot, AdminMailError, AdminMailIntakeDetail, AdminMailIntakeFilter,
    AdminMailIntakePage, AdminMailPollState, AdminMailPollStateRequest, AdminMailReader,
    ApplicationError, ApplicationFuture,
};
#[cfg(feature = "postgres")]
use auction_application::{
    AdminMailIntakeDelivery, AdminMailIntakeMetadata, AdminMailIntakeSummary,
    AdminMailPollTriggerCommand, AdminMailPollerSnapshot, AdminMailPollingCommand,
    AdminMailStoredCommand, AdminMailTransaction, AdminMailTransactionError,
    AdminMailTransactionFailure, AdminMailTransactionRunner, MAX_ADMIN_MAIL_POLLER_COUNT,
    RequestHash, TransactionFuture, TransactionPolicy,
};
#[cfg(feature = "postgres")]
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use serde::Deserialize;
#[cfg(feature = "postgres")]
use serde_json::Value;
#[cfg(feature = "postgres")]
use sqlx::Row;
#[cfg(feature = "postgres")]
use sqlx::{Postgres, Transaction, types::Json};
#[cfg(feature = "postgres")]
use std::future::Future;
use uuid::Uuid;

#[cfg(feature = "postgres")]
use crate::database::audit_health::AuditWriteResult as _;

use super::DatabaseStore;

#[cfg(feature = "postgres")]
mod backfill;
#[cfg(feature = "postgres")]
pub use backfill::{MailDisplayBackfillInput, MailDisplayBackfillResult};
#[cfg(feature = "postgres")]
mod mapping;
#[cfg(feature = "postgres")]
mod reads;
#[cfg(feature = "postgres")]
mod transaction;

impl AdminMailReader for DatabaseStore {
    fn dashboard_snapshot(&self) -> ApplicationFuture<'_, AdminMailDashboardSnapshot> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => reads::dashboard_snapshot_postgres(store).await,
            }
        })
    }

    fn poll_state(
        &self,
        request: AdminMailPollStateRequest,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        Box::pin(async move {
            request.validate().map_err(ApplicationError::from)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    reads::get_poll_state_postgres(store, request).await
                }
            }
        })
    }

    fn list_intakes(
        &self,
        request: AdminMailIntakeFilter,
    ) -> ApplicationFuture<'_, AdminMailIntakePage> {
        Box::pin(async move {
            request.validate().map_err(ApplicationError::from)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    reads::list_intakes_postgres(store, request).await
                }
            }
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
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    reads::get_intake_postgres(store, intake_id).await
                }
            }
        })
    }
}
