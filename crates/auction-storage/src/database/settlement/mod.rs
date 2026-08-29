use std::future::Future;

use auction_application::{
    CommandLedgerError, PersistedSettlement, PersistedSettlementObligationCount,
    SettlementSnapshot, SettlementTransaction, SettlementTransactionError,
    SettlementTransactionFailure, SettlementTransactionRunner, StoredCommand, TransactionFuture,
    TransactionIsolation, TransactionPolicy,
};
use auction_domain::{
    AuctionId, CommandId, CompetitionPlan, ProxyStatus, RecoveryPolicy, SettlementAmount,
    SettlementFact, SettlementLine, SettlementMode, SettlementObligation, SettlementObligationKind,
    SettlementObligationState, SettlementPlan, SettlementPolicy, SettlementWorld,
    deterministic_uuid,
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::database::audit_health::AuditWriteResult as _;

use super::{PostgresCompetitionTransaction, PostgresStore};
#[path = "facts.rs"]
mod facts;
#[path = "mapping.rs"]
mod mapping;
#[path = "obligations.rs"]
mod obligations;
#[path = "transaction.rs"]
mod transaction;

use facts::*;
use mapping::*;
use obligations::*;

/// PostgreSQL 结算事务 runner。结算与竞价都使用 Serializable，但锁集合和落表步骤保持独立。
#[allow(clippy::manual_async_fn)]
impl SettlementTransactionRunner for PostgresStore {
    type Transaction = PostgresCompetitionTransaction;

    fn run<T, E, F>(
        &self,
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: SettlementTransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send,
    {
        async move {
            if let Err(error) = TransactionPolicy::new(policy.isolation, policy.max_attempts) {
                return Err(E::from(error));
            }

            for attempt in 0..policy.max_attempts {
                let transaction = match self.pool().begin().await {
                    Ok(transaction) => transaction,
                    Err(_) => {
                        return Err(E::from(SettlementTransactionError::Failed(
                            "PostgreSQL 结算事务无法开始".to_owned(),
                        )));
                    }
                };
                let mut transaction = PostgresCompetitionTransaction::new(transaction);

                if let Err(error) = set_isolation(&mut transaction, policy.isolation).await {
                    if matches!(error, SettlementTransactionError::SerializationConflict)
                        && attempt + 1 < policy.max_attempts
                    {
                        continue;
                    }
                    return Err(E::from(error));
                }

                match operation(&mut transaction).await {
                    Ok(value) => match transaction.commit().await {
                        Ok(()) => return Ok(value),
                        Err(_) if attempt + 1 < policy.max_attempts => continue,
                        Err(_) => {
                            return Err(E::from(SettlementTransactionError::Failed(
                                "PostgreSQL 结算事务提交失败".to_owned(),
                            )));
                        }
                    },
                    Err(error) => {
                        if error.is_serialization_conflict() && attempt + 1 < policy.max_attempts {
                            continue;
                        }
                        return Err(error);
                    }
                }
            }

            Err(E::from(SettlementTransactionError::SerializationConflict))
        }
    }
}

#[allow(clippy::manual_async_fn)]
impl SettlementTransaction for PostgresCompetitionTransaction {
    fn find_command(
        &mut self,
        command_id: CommandId,
    ) -> impl Future<Output = Result<Option<StoredCommand>, SettlementTransactionError>> + Send
    {
        async move { self.find_command_inner(command_id).await.map_err(map_competition_error) }
    }

    fn load_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<SettlementSnapshot, SettlementTransactionError>> + Send {
        async move { self.load_settlement_for_update(auction_id).await }
    }

    fn insert_command(
        &mut self,
        command: StoredCommand,
        occurred_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), SettlementTransactionError>> + Send {
        async move { self.insert_settlement_command(command, occurred_at).await }
    }

    fn apply_settlement(
        &mut self,
        auction_id: AuctionId,
        command_id: CommandId,
        plan: SettlementPlan,
        proxy_closure_plans: Vec<CompetitionPlan>,
        recovery_policy: RecoveryPolicy,
        occurred_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), SettlementTransactionError>> + Send {
        async move {
            self.apply_settlement_inner(
                auction_id,
                command_id,
                plan,
                proxy_closure_plans,
                recovery_policy,
                occurred_at,
            )
            .await
        }
    }
}

async fn set_isolation(
    transaction: &mut PostgresCompetitionTransaction,
    isolation: TransactionIsolation,
) -> Result<(), SettlementTransactionError> {
    let statement = match isolation {
        TransactionIsolation::Serializable => "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
        TransactionIsolation::ReadCommitted => "SET TRANSACTION ISOLATION LEVEL READ COMMITTED",
    };
    let transaction = transaction.transaction_mut().map_err(map_competition_error)?;
    sqlx::query(statement).execute(&mut **transaction).await.map_err(map_sql_error)?;
    Ok(())
}
