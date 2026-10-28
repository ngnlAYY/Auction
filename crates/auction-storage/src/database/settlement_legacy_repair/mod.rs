//! 旧自寄修复的唯一 SQL owner；预览无副作用，显式执行借用真实证据并追加永久恢复事实。

mod proof;
mod read;
mod write;

use auction_application::{
    FundingError, FundingFuture, LegacySelfReconciliationCommand, LegacySelfReconciliationOutcome,
    LegacySelfReconciliationReader, LegacySelfReconciliationResult, LegacySelfReconciliationTarget,
    LegacySelfReconciliationWriter, SettlementInternalEvidenceSnapshot,
};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::DatabaseStore;

pub(crate) use proof::{TRUSTED_DELIVERY_PREDICATE, legacy_self_uid};

fn sql_error(error: sqlx::Error) -> FundingError {
    if crate::database::is_serialization_conflict(&error) {
        FundingError::SerializationConflict
    } else {
        FundingError::Storage
    }
}

fn evidence_error(error: crate::MailEvidenceTransactionError) -> FundingError {
    match error {
        crate::MailEvidenceTransactionError::Sqlx(error) => sql_error(error),
        crate::MailEvidenceTransactionError::Rejected(_) => FundingError::Unproven,
    }
}

impl LegacySelfReconciliationReader for DatabaseStore {
    fn legacy_self_command_target(
        &self,
        command_id: Uuid,
    ) -> FundingFuture<'_, Option<LegacySelfReconciliationTarget>> {
        Box::pin(async move {
            if command_id.is_nil() {
                return Err(FundingError::Invalid);
            }
            let value: Option<serde_json::Value> = sqlx::query_scalar(
                "SELECT original_snapshot->'target' FROM public.settlement_legacy_self_reconciliations WHERE command_id=$1"
            ).bind(command_id).fetch_optional(self.postgres_pool()).await.map_err(sql_error)?;
            value
                .map(|value| serde_json::from_value(value).map_err(|_| FundingError::Invalid))
                .transpose()
        })
    }

    fn preview_legacy_self(
        &self,
        obligation_id: Uuid,
    ) -> FundingFuture<'_, LegacySelfReconciliationTarget> {
        Box::pin(async move {
            let mut tx = self
                .postgres_pool()
                .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE, READ ONLY")
                .await
                .map_err(sql_error)?;
            let (_, target) = read::snapshot(&mut tx, obligation_id).await?;
            tx.commit().await.map_err(sql_error)?;
            Ok(target)
        })
    }

    fn replay_legacy_self(
        &self,
        command: LegacySelfReconciliationCommand,
    ) -> FundingFuture<'_, Option<LegacySelfReconciliationResult>> {
        Box::pin(async move {
            command.validate()?;
            let mut tx = self.postgres_pool().begin().await.map_err(sql_error)?;
            let result = read::replay(&mut tx, &command).await?;
            tx.commit().await.map_err(sql_error)?;
            Ok(result)
        })
    }
}

impl LegacySelfReconciliationWriter for DatabaseStore {
    fn reconcile_legacy_self<'a>(
        &'a self,
        command: &'a LegacySelfReconciliationCommand,
        evidence: &'a SettlementInternalEvidenceSnapshot,
    ) -> FundingFuture<'a, LegacySelfReconciliationOutcome> {
        Box::pin(write::run(self.postgres_pool(), command, evidence))
    }
}

/// 低频修复与资金对账使用相同全局行锁顺序，不把当前 manual 行改成可被外发领取的任务。
async fn lock_rows(
    tx: &mut Transaction<'_, Postgres>,
    query: &'static str,
) -> Result<(), FundingError> {
    sqlx::query(query).execute(&mut **tx).await.map_err(sql_error)?;
    Ok(())
}
