pub(crate) mod catalog;
pub(crate) mod commands;
pub(crate) mod compensation_reversal;
pub(crate) mod fulfillment;
pub(crate) mod funding;
pub(crate) mod policy;
pub(crate) mod report;
pub(crate) mod scheduler;

use std::future::Future;

use auction_application::core::{StoredCommand, TransactionFuture, TransactionPolicy};
use auction_application::settlement::{
    SettlementSnapshot, SettlementTransaction, SettlementTransactionError,
    SettlementTransactionFailure, SettlementTransactionRunner,
};
use auction_domain::{AuctionId, CommandId, CompetitionPlan, RecoveryPolicy, SettlementPlan};
use chrono::{DateTime, Utc};

use crate::database::auction::competition::begin_statement;

use super::{DatabaseStore, PostgresCompetitionTransaction};
mod facts;
mod mapping;
mod obligations;
mod transaction;

use facts::*;
use mapping::*;
use obligations::*;

/// 结算批量 INSERT 每条语句的最大行数；最宽的义务行也远低于 PostgreSQL 65535 个参数上限。
const SETTLEMENT_BATCH_SIZE: usize = 256;

/// 在原 Serializable 事务内执行一个结算批次，减少持有结算锁期间的往返。完整批次复用固定语句形状，
/// 可变尾批不挤占连接的预处理语句缓存。
async fn execute_settlement_batch(
    transaction: &mut crate::database::TrackedTransaction<'static, sqlx::Postgres>,
    mut query: sqlx::QueryBuilder<sqlx::Postgres>,
    rows: usize,
) -> Result<(), SettlementTransactionError> {
    query
        .build()
        .persistent(rows == SETTLEMENT_BATCH_SIZE)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    Ok(())
}

/// PostgreSQL 结算事务 runner。结算与竞价都使用 Serializable，但锁集合和落表步骤保持独立。
#[allow(clippy::manual_async_fn)]
impl SettlementTransactionRunner for DatabaseStore {
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
        self.pool().observe_transaction(
            auction_observability::DatabaseTransactionOwner::Settlement,
            async move {
                if let Err(error) = TransactionPolicy::new(policy.isolation, policy.max_attempts) {
                    return Err(E::from(error));
                }

                for attempt in 0..policy.max_attempts {
                    let transaction =
                        match self.pool().begin_with(begin_statement(policy.isolation)).await {
                            Ok(transaction) => transaction,
                            Err(_) => {
                                return Err(E::from(SettlementTransactionError::Failed(
                                    "PostgreSQL 结算事务无法开始".to_owned(),
                                )));
                            }
                        };
                    let mut transaction = PostgresCompetitionTransaction::new(transaction);

                    match operation(&mut transaction).await {
                        Ok(value) => {
                            let lot_watch_events = transaction.take_lot_watch_events();
                            match transaction.commit().await {
                                Ok(()) => {
                                    crate::database::lot_watch::process_committed_events(
                                        self,
                                        &lot_watch_events,
                                    )
                                    .await;
                                    return Ok(value);
                                }
                                Err(error) => {
                                    let error = map_sql_error(error);
                                    if matches!(
                                        error,
                                        SettlementTransactionError::SerializationConflict
                                    ) && attempt + 1 < policy.max_attempts
                                    {
                                        crate::database::serializable_retry_backoff(usize::from(
                                            attempt + 1,
                                        ))
                                        .await;
                                        continue;
                                    }
                                    crate::database::observe_retry_exhaustion(
                                        usize::from(attempt + 1),
                                        usize::from(policy.max_attempts),
                                        matches!(
                                            error,
                                            SettlementTransactionError::SerializationConflict
                                        ),
                                    );
                                    return Err(E::from(error));
                                }
                            }
                        }
                        Err(error) => {
                            if error.is_serialization_conflict()
                                && attempt + 1 < policy.max_attempts
                            {
                                crate::database::serializable_retry_backoff(usize::from(
                                    attempt + 1,
                                ))
                                .await;
                                continue;
                            }
                            crate::database::observe_retry_exhaustion(
                                usize::from(attempt + 1),
                                usize::from(policy.max_attempts),
                                error.is_serialization_conflict(),
                            );
                            return Err(error);
                        }
                    }
                }

                Err(E::from(SettlementTransactionError::SerializationConflict))
            },
        )
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
        plan: &SettlementPlan,
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
