//! 内部满足事务装配；资金 SQL 唯一拥有者仍为 settlement_funding。

use auction_application::{
    ApplicationFuture, FundingFuture, SettlementFulfillmentProof, SettlementInternalCommand,
    SettlementInternalCommitOutcome, SettlementInternalEvidenceSnapshot,
    SettlementInternalFulfillmentStore,
};

#[cfg(feature = "postgres")]
use auction_application::FundingError;

#[cfg(feature = "postgres")]
mod conflicts;
#[cfg(feature = "postgres")]
mod graph;
#[cfg(feature = "postgres")]
mod locks;
#[cfg(feature = "postgres")]
mod persistence;
#[cfg(feature = "postgres")]
mod transaction;
#[cfg(feature = "postgres")]
mod validation;

#[cfg(feature = "postgres")]
pub(crate) use conflicts::{assetless_mail_notification, exclude_legacy_self_asset_conflicts};

impl SettlementInternalFulfillmentStore for crate::DatabaseStore {
    fn legacy_self_reconciliation_uid(
        &self,
        obligation_id: uuid::Uuid,
    ) -> ApplicationFuture<'_, Option<String>> {
        Box::pin(async move {
            let _ = obligation_id;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    let mut tx =
                        store.pool().begin().await.map_err(|_| {
                            super::mapping::repository_error("旧自寄修复证明不可用")
                        })?;
                    let result = crate::database::settlement_legacy_repair::legacy_self_uid(
                        &mut tx,
                        obligation_id,
                    )
                    .await
                    .map_err(|_| super::mapping::repository_error("旧自寄修复证明不匹配"))?;
                    tx.commit()
                        .await
                        .map_err(|_| super::mapping::repository_error("旧自寄修复证明读取失败"))?;
                    Ok(result)
                }
            }
        })
    }
    fn replay_internal(&self, command: SettlementInternalCommand) -> ApplicationFuture<'_, bool> {
        Box::pin(async move {
            let _ = &command;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => persistence::replay_pool(store.pool(), &command)
                    .await
                    .map_err(|error| match error {
                        FundingError::Conflict => {
                            auction_application::ApplicationError::FulfillmentDependencyMismatch
                        }
                        _ => super::mapping::repository_error("内部命令账本暂不可用"),
                    }),
            }
        })
    }

    fn satisfy_internal<'a>(
        &'a self,
        command: &'a SettlementInternalCommand,
        evidence: &'a SettlementInternalEvidenceSnapshot,
    ) -> FundingFuture<'a, SettlementInternalCommitOutcome> {
        Box::pin(async move {
            let _ = (command, evidence);
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    transaction::run(store.pool(), command, evidence, None, false).await
                }
            }
        })
    }

    fn prepare_external_refund<'a>(
        &'a self,
        command: &'a SettlementInternalCommand,
        evidence: &'a SettlementInternalEvidenceSnapshot,
    ) -> FundingFuture<'a, SettlementInternalCommitOutcome> {
        Box::pin(async move {
            let _ = (command, evidence);
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    transaction::run(store.pool(), command, evidence, None, true).await
                }
            }
        })
    }

    fn satisfy_collected<'a>(
        &'a self,
        command: &'a SettlementInternalCommand,
        proof: &'a SettlementFulfillmentProof,
        evidence: &'a SettlementInternalEvidenceSnapshot,
    ) -> FundingFuture<'a, SettlementInternalCommitOutcome> {
        Box::pin(async move {
            let _ = (command, proof, evidence);
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    transaction::run(store.pool(), command, evidence, Some(proof), false).await
                }
            }
        })
    }
}

/// 旧prepare/activate/satisfy也遵守同一业务锁序，不能在fulfillment后反锁义务。
#[cfg(feature = "postgres")]
pub(super) async fn lock_legacy(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &auction_application::SettlementFulfillmentClaim,
) -> Result<bool, FundingError> {
    locks::worlds(tx).await?;
    let protected = crate::settlement_funding::funding_requires_authorization(tx).await?;
    locks::before_credentials(tx, claim).await?;
    locks::after_credentials(tx, claim).await?;
    if let Some(user_id) = claim.recipient_user_id {
        let current: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.users WHERE id=$1 AND ($3 OR (username=$2 AND status='active')))")
            .bind(user_id).bind(&claim.recipient)
            .bind(claim.state == auction_application::SettlementFulfillmentState::Enqueued)
            .fetch_one(&mut **tx).await.map_err(sql_error)?;
        if !current {
            return Err(FundingError::Conflict);
        }
    }
    Ok(protected)
}

#[cfg(feature = "postgres")]
fn sql_error(error: sqlx::Error) -> FundingError {
    if error
        .as_database_error()
        .and_then(|e| e.code())
        .is_some_and(|code| matches!(code.as_ref(), "40001" | "40P01"))
    {
        FundingError::SerializationConflict
    } else {
        FundingError::Storage
    }
}

#[cfg(feature = "postgres")]
fn evidence_error(error: crate::MailEvidenceTransactionError) -> FundingError {
    match error {
        crate::MailEvidenceTransactionError::Sqlx(error) => sql_error(error),
        crate::MailEvidenceTransactionError::Rejected(_) => FundingError::Unproven,
    }
}
