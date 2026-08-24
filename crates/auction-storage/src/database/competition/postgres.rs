use std::{collections::HashSet, future::Future};

use auction_application::{
    AdminAuctionCreateTransaction, AdminAuctionSummary, AdminCreateAuctionCommand, AdminLotSummary,
    AdminLotUpdateCommand, AdminLotUpdateSnapshot, AdminLotUpdateTransaction,
    AuctionLifecycleCommand, AuctionLifecycleContext, AuctionLifecycleEffect, AuctionLifecyclePlan,
    AuctionLifecycleResult, AuctionLifecycleTransaction, AuctionPolicyTemplate,
    AuctionPreviewSuccessorPlan, CommandLedgerError, CompetitionSnapshot, CompetitionTransaction,
    CompetitionTransactionError, RequestHash, SettlementAuctionSnapshot, StoredCommand,
    TransactionFailure, TransactionFuture, TransactionIsolation, TransactionPolicy,
    TransactionRunner,
};
use auction_domain::{
    AcceptedBid, Auction, AuctionId, AuctionRules, AuctionStatus, BidAmount, BidMechanism,
    CompetitionPlan, LotBidState, LotId, ProxyAuthorization, ProxyAuthorizationId, ProxyMode,
    ProxyStatus, ProxyTerminalReason, UserId, deterministic_uuid,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;

use crate::database::audit_health::AuditWriteResult as _;

use super::PostgresStore;
#[path = "postgres/admin.rs"]
mod admin;
#[path = "postgres/bidding.rs"]
mod bidding;
#[path = "postgres/lifecycle.rs"]
mod lifecycle;
#[path = "postgres/mapping.rs"]
mod mapping;

pub(crate) use mapping::*;

type PgTransaction = Transaction<'static, Postgres>;

/// PostgreSQL 竞价事务的唯一实现。
///
/// 事务对象拥有 SQLx 从连接池取得的连接，因此应用回调结束后 runner 仍能提交或回滚；
/// 回调不能拿到独立连接，也不能把命令账本和竞价写入拆成两个事务。
pub struct PostgresCompetitionTransaction {
    transaction: Option<PgTransaction>,
}

impl PostgresCompetitionTransaction {
    pub(crate) fn new(transaction: PgTransaction) -> Self {
        Self { transaction: Some(transaction) }
    }

    pub(crate) async fn commit(mut self) -> Result<(), sqlx::Error> {
        match self.transaction.take() {
            Some(transaction) => transaction.commit().await,
            None => Ok(()),
        }
    }

    pub(crate) fn transaction_mut(
        &mut self,
    ) -> Result<&mut PgTransaction, CompetitionTransactionError> {
        self.transaction
            .as_mut()
            .ok_or_else(|| CompetitionTransactionError::Failed("竞价事务已经结束".to_owned()))
    }
}

// 公开事务端口使用 impl Future 以保持适配器不依赖宏；这里必须保留同一签名。
#[allow(clippy::manual_async_fn)]
impl TransactionRunner for PostgresStore {
    type Transaction = PostgresCompetitionTransaction;

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

            for attempt in 0..policy.max_attempts {
                let transaction = match self.pool().begin().await {
                    Ok(transaction) => transaction,
                    Err(error) => return Err(E::from(map_sql_error(error))),
                };
                let mut transaction = PostgresCompetitionTransaction::new(transaction);

                if let Err(error) =
                    set_isolation(transaction.transaction_mut(), policy.isolation).await
                {
                    let retryable =
                        matches!(error, CompetitionTransactionError::SerializationConflict);
                    if retryable && attempt + 1 < policy.max_attempts {
                        continue;
                    }
                    return Err(E::from(error));
                }

                match operation(&mut transaction).await {
                    Ok(value) => match transaction.commit().await {
                        Ok(()) => return Ok(value),
                        Err(error) => {
                            let error = map_sql_error(error);
                            if matches!(error, CompetitionTransactionError::SerializationConflict)
                                && attempt + 1 < policy.max_attempts
                            {
                                continue;
                            }
                            return Err(E::from(error));
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

            Err(E::from(CompetitionTransactionError::SerializationConflict))
        }
    }
}

// 公开事务端口使用 impl Future；具体适配器用 async block 绑定借用事务的生命周期。
#[allow(clippy::manual_async_fn)]
impl CompetitionTransaction for PostgresCompetitionTransaction {
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
