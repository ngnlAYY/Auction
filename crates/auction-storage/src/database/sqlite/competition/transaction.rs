//! SQLite Lite 档竞价事务。
//!
//! SQLite 没有 `FOR UPDATE`。整个写路径先取得连接并执行 `BEGIN IMMEDIATE`，
//! 由 SQLite 的单写者锁保护 lot、代理、命令账本和事件写入。

use std::future::Future;

use auction_application::{
    AuctionLifecycleCommand, AuctionLifecycleContext, AuctionLifecycleEffect, AuctionLifecyclePlan,
    AuctionLifecycleResult, AuctionLifecycleTransaction, AuctionPreviewSuccessorPlan,
    CommandLedgerError, CompetitionSnapshot, CompetitionTransaction, CompetitionTransactionError,
    RequestHash, StoredCommand, TransactionFailure, TransactionFuture, TransactionIsolation,
    TransactionPolicy, TransactionRunner,
};
use auction_domain::{
    AcceptedBid, Auction, AuctionId, AuctionRules, AuctionStatus, BidAmount, BidMechanism,
    CompetitionPlan, LotBidState, LotId, ProxyAuthorization, ProxyAuthorizationId, ProxyMode,
    ProxyStatus, ProxyTerminalReason, UserId,
};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use serde_json::{Value, json};
use sqlx::{Row, Sqlite, SqliteConnection, pool::PoolConnection, sqlite::SqliteRow};
use uuid::Uuid;

use crate::database::audit_health::AuditWriteResult as _;

use super::super::SqliteStore;
#[path = "transaction/bidding/mod.rs"]
mod bidding;
#[path = "transaction/lifecycle.rs"]
mod lifecycle;
#[path = "transaction/mapping.rs"]
mod mapping;

pub(crate) use mapping::*;

/// SQLite 竞价事务持有的唯一连接。
///
/// 连接在 `BEGIN IMMEDIATE` 到提交/回滚之间不能交还连接池，避免同一命令的账本、
/// lot 状态和追加事实被拆到不同事务。
pub struct SqliteCompetitionTransaction {
    connection: Option<PoolConnection<Sqlite>>,
}

impl SqliteCompetitionTransaction {
    async fn begin(connection: PoolConnection<Sqlite>) -> Result<Self, sqlx::Error> {
        let mut transaction = Self { connection: Some(connection) };
        sqlx::query("BEGIN IMMEDIATE")
            .execute(
                transaction
                    .connection_mut()
                    .map_err(|_| sqlx::Error::Protocol("SQLite 事务连接不可用".to_owned()))?,
            )
            .await?;
        Ok(transaction)
    }

    async fn commit(mut self) -> Result<(), sqlx::Error> {
        let Some(mut connection) = self.connection.take() else {
            return Ok(());
        };
        let result = sqlx::query("COMMIT").execute(&mut *connection).await;
        if result.is_err() {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
        }
        result.map(|_| ())
    }

    async fn rollback(mut self) -> Result<(), sqlx::Error> {
        let Some(mut connection) = self.connection.take() else {
            return Ok(());
        };
        sqlx::query("ROLLBACK").execute(&mut *connection).await.map(|_| ())
    }

    fn connection_mut(&mut self) -> Result<&mut SqliteConnection, CompetitionTransactionError> {
        self.connection
            .as_deref_mut()
            .ok_or_else(|| CompetitionTransactionError::Failed("竞价事务已经结束".to_owned()))
    }
}

/// SQLite 使用 `BEGIN IMMEDIATE` 把 Serializable 竞价写入映射为单写者事务。
#[allow(clippy::manual_async_fn)]
impl TransactionRunner for SqliteStore {
    type Transaction = SqliteCompetitionTransaction;

    fn run<T, E, F>(
        &self,
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: TransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send,
    {
        async move {
            if let Err(error) = TransactionPolicy::new(policy.isolation, policy.max_attempts) {
                return Err(E::from(error));
            }

            // Lite 档的两个隔离枚举都由同一个 SQLite 写锁入口承载；不在这里伪造
            // PostgreSQL 的隔离级别，而是保留应用层策略并显式执行 IMMEDIATE。
            let _isolation = match policy.isolation {
                TransactionIsolation::Serializable => "serializable",
                TransactionIsolation::ReadCommitted => "read_committed",
            };

            for attempt in 0..policy.max_attempts {
                let connection = match self.pool().acquire().await {
                    Ok(connection) => connection,
                    Err(error) => {
                        let mapped = map_sql_error(error);
                        if matches!(mapped, CompetitionTransactionError::SerializationConflict)
                            && attempt + 1 < policy.max_attempts
                        {
                            continue;
                        }
                        return Err(E::from(mapped));
                    }
                };
                let mut transaction = match Self::Transaction::begin(connection).await {
                    Ok(transaction) => transaction,
                    Err(error) => {
                        let mapped = map_sql_error(error);
                        if matches!(mapped, CompetitionTransactionError::SerializationConflict)
                            && attempt + 1 < policy.max_attempts
                        {
                            continue;
                        }
                        return Err(E::from(mapped));
                    }
                };

                match operation(&mut transaction).await {
                    Ok(value) => match transaction.commit().await {
                        Ok(()) => return Ok(value),
                        Err(error) => {
                            let mapped = map_sql_error(error);
                            if matches!(mapped, CompetitionTransactionError::SerializationConflict)
                                && attempt + 1 < policy.max_attempts
                            {
                                continue;
                            }
                            return Err(E::from(mapped));
                        }
                    },
                    Err(error) => {
                        let retryable = error.is_serialization_conflict();
                        let _ = transaction.rollback().await;
                        if retryable && attempt + 1 < policy.max_attempts {
                            continue;
                        }
                        return Err(error);
                    }
                }
            }

            Err(E::from(CompetitionTransactionError::SerializationConflict))
        }
    }
}

#[allow(clippy::manual_async_fn)]
impl CompetitionTransaction for SqliteCompetitionTransaction {
    fn find_command(
        &mut self,
        command_id: auction_domain::CommandId,
    ) -> impl Future<Output = Result<Option<StoredCommand>, CompetitionTransactionError>> + Send
    {
        async move { self.find_command_inner(command_id).await }
    }

    fn insert_command(
        &mut self,
        command: StoredCommand,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { self.insert_command_inner(command).await }
    }

    fn load_for_update(
        &mut self,
        lot_id: LotId,
    ) -> impl Future<Output = Result<CompetitionSnapshot, CompetitionTransactionError>> + Send {
        async move { self.load_for_update_inner(lot_id).await }
    }

    fn next_proxy_usage_number(
        &mut self,
        lot_id: LotId,
        user_id: UserId,
    ) -> impl Future<Output = Result<u64, CompetitionTransactionError>> + Send {
        async move { self.next_proxy_usage_number_inner(lot_id, user_id).await }
    }

    fn apply_plan(
        &mut self,
        plan: CompetitionPlan,
        request_id: Option<String>,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { self.apply_plan_inner(plan, request_id).await }
    }
}
