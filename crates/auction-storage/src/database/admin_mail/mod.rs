//! Admin 邮件轮询状态与入站审查的 PostgreSQL 适配器。
//!
//! `inbound_mail` 不保存原始密文；详情只从批准字段和经过脱敏、限界校验的展示投影生成。
//! SQLite Lite 没有这些完整档邮件表，因此所有对应能力显式返回 501。

use auction_application::{
    AdminMailDashboardSnapshot, AdminMailError, AdminMailIntakeDetail, AdminMailIntakeFilter,
    AdminMailIntakePage, AdminMailPollState, AdminMailPollStateRequest, AdminMailReader,
    ApplicationError, ApplicationFuture,
};
#[cfg(feature = "postgres")]
use auction_application::{
    AdminMailIntakeMetadata, AdminMailIntakeSummary, AdminMailPollTriggerCommand,
    AdminMailPollerSnapshot, AdminMailPollingCommand, AdminMailStoredCommand, AdminMailTransaction,
    AdminMailTransactionError, AdminMailTransactionFailure, AdminMailTransactionRunner,
    MAX_ADMIN_MAIL_POLLER_COUNT, RequestHash, TransactionFuture, TransactionIsolation,
    TransactionPolicy,
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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }
}

#[cfg(feature = "sqlite")]
fn capability_error() -> ApplicationError {
    ApplicationError::CapabilityUnavailable("Admin 邮件状态与入站审查仅支持 PostgreSQL 完整档")
}
