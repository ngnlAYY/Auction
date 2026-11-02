//! 人工 Sent 责任事实与原投递同事务绑定；现金只收口原未知支出，不创建来源或收款。

use auction_application::{
    ApplicationError, ApplicationFuture, DeliverySendExpectation, DeliverySentConfirmationPort,
    FundingError, ManualSentEvidence, SentConfirmationCommand, SentConfirmationPreview,
    SentConfirmationResult,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use super::{DatabaseStore, delivery_scheduler::attempts, settlement_funding as funding};

mod snapshot;
mod transaction;

enum Error {
    Sql(sqlx::Error),
    Funding(FundingError),
    Application(ApplicationError),
    Json(serde_json::Error),
    Rejected,
}

impl Error {
    fn retryable(&self) -> bool {
        match self {
            Self::Sql(error) => super::is_serialization_conflict(error),
            Self::Funding(FundingError::SerializationConflict) => true,
            _ => false,
        }
    }

    fn application(self) -> ApplicationError {
        match self {
            Self::Application(error) => error,
            Self::Json(error) => ApplicationError::Serialization(error),
            Self::Rejected => ApplicationError::FulfillmentDependencyMismatch,
            Self::Sql(_) | Self::Funding(_) => ApplicationError::FulfillmentDependencyUnavailable,
        }
    }
}

impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        Self::Sql(error)
    }
}
impl From<FundingError> for Error {
    fn from(error: FundingError) -> Self {
        Self::Funding(error)
    }
}
impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl DeliverySentConfirmationPort for DatabaseStore {
    fn preview_sent_confirmation(
        &self,
        delivery_id: Uuid,
        evidence: ManualSentEvidence,
    ) -> ApplicationFuture<'_, SentConfirmationPreview> {
        Box::pin(async move {
            evidence.validate()?;
            if delivery_id.is_nil() {
                return Err(ApplicationError::FulfillmentDependencyMismatch);
            }
            let operation = async {
                let mut tx = self
                    .postgres_pool()
                    .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
                    .await?;
                sqlx::query("SET LOCAL statement_timeout='15s'").execute(&mut *tx).await?;
                let snapshot = snapshot::load(&mut tx, delivery_id, &evidence).await?;
                tx.rollback().await?;
                Ok::<_, Error>(snapshot.preview)
            }
            .await;
            operation.map_err(Error::application)
        })
    }

    fn confirm_sent(
        &self,
        command: SentConfirmationCommand,
    ) -> ApplicationFuture<'_, SentConfirmationResult> {
        Box::pin(async move {
            command.validate()?;
            let command = &command;
            super::retry_conflicts(super::SERIALIZABLE_ATTEMPTS, Error::retryable, || async move {
                let mut tx =
                    self.postgres_pool().begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
                sqlx::query("SET LOCAL transaction_timeout='30s'").execute(&mut *tx).await?;
                super::lock_command_id(&mut tx, command.command_id).await?;
                if let Some(result) = transaction::replay(&mut tx, command).await? {
                    tx.commit().await?;
                    return Ok(result);
                }
                let result = transaction::apply(&mut tx, command).await?;
                tx.commit().await?;
                Ok::<_, Error>(result)
            })
            .await
            .map_err(Error::application)
        })
    }
}
