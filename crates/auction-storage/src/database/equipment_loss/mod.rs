//! 装备损失人工处置的数据库端口；不接触外部系统，不改写原经济结算或补偿账本。

use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::settlement::{
    EquipmentLossCommand, EquipmentLossPreview, EquipmentLossResult, FundingError,
    SettlementEquipmentLossPort,
};
use uuid::Uuid;

use super::DatabaseStore;

mod proof;
#[cfg(feature = "test-db")]
pub use proof::delivery_snapshot_equality_sql;
pub(crate) use proof::trusted_losses;
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
            Self::Sql(error) => super::observed_serialization_conflict(error),
            Self::Funding(FundingError::SerializationConflict) => true,
            _ => false,
        }
    }
    fn application(self) -> ApplicationError {
        match self {
            Self::Application(error) => error,
            Self::Json(error) => ApplicationError::Serialization(error),
            Self::Rejected
            | Self::Funding(
                FundingError::Invalid
                | FundingError::Conflict
                | FundingError::Unproven
                | FundingError::Unsupported,
            ) => ApplicationError::FulfillmentDependencyMismatch,
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

impl SettlementEquipmentLossPort for DatabaseStore {
    fn preview_equipment_loss(&self, lot_id: Uuid) -> ApplicationFuture<'_, EquipmentLossPreview> {
        Box::pin(async move {
            if lot_id.is_nil() {
                return Err(ApplicationError::FulfillmentDependencyMismatch);
            }
            let operation = async {
                let mut tx = self
                    .tracked_postgres_pool()
                    .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
                    .await?;
                sqlx::query("SET LOCAL statement_timeout='15s'").execute(&mut *tx).await?;
                let snapshot = snapshot::load(&mut tx, lot_id).await?;
                tx.rollback().await?;
                Ok::<_, Error>(snapshot.preview)
            }
            .await;
            operation.map_err(Error::application)
        })
    }

    fn resolve_equipment_loss(
        &self,
        command: EquipmentLossCommand,
    ) -> ApplicationFuture<'_, EquipmentLossResult> {
        Box::pin(async move {
            command.validate()?;
            let command = &command;
            super::retry_conflicts(
                self.tracked_postgres_pool(),
                auction_observability::DatabaseTransactionOwner::EquipmentLoss,
                super::SERIALIZABLE_ATTEMPTS,
                Error::retryable,
                || async move {
                    let mut tx = self
                        .tracked_postgres_pool()
                        .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
                        .await?;
                    sqlx::query("SET LOCAL transaction_timeout='30s'").execute(&mut *tx).await?;
                    super::lock_command_id(&mut tx, command.command_id).await?;
                    if let Some(result) = transaction::replay(&mut tx, command).await? {
                        tx.commit().await?;
                        return Ok(result);
                    }
                    let result = transaction::apply(&mut tx, command).await?;
                    tx.commit().await?;
                    Ok::<_, Error>(result)
                },
            )
            .await
            .map_err(Error::application)
        })
    }
}
