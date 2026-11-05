use crate::database::TrackedTransaction as Transaction;
use std::future::Future;

use auction_application::{
    AdminAuctionCreateTransaction, AdminAuctionScheduleSnapshot, AdminAuctionScheduleTransaction,
    AdminAuctionSummary, AdminCreateAuctionCommand, AdminLotSummary, AdminLotUpdateCommand,
    AdminLotUpdateSnapshot, AdminLotUpdateTransaction, AdminUpdateAuctionScheduleCommand,
    AdminWorldSetEnabledCommand, AdminWorldTransaction, AuctionLifecycleCommand,
    AuctionLifecycleContext, AuctionLifecycleEffect, AuctionLifecyclePlan, AuctionLifecycleResult,
    AuctionLifecycleTransaction, AuctionLifecycleWriteSummary, AuctionPolicyTemplate,
    AuctionPreviewSuccessorPlan, CommandInsert, CommandLedgerError, CompetitionCommandAuditContext,
    CompetitionProxyTransitionRecord, CompetitionSnapshot, CompetitionTransaction,
    CompetitionTransactionError, ProxyUsageContext, RequestHash, SettlementAuctionSnapshot,
    SlowProxyExecutionContext, StoredCommand, TransactionFailure, TransactionFuture,
    TransactionIsolation, TransactionPolicy, TransactionRunner, WorldSnapshot,
};
use auction_domain::{
    AcceptedBid, Auction, AuctionId, AuctionRules, AuctionStatus, BidAmount, CompetitionPlan,
    LotBidState, LotId, ProxyAuthorization, ProxyAuthorizationId, ProxyMode, ProxyStatus,
    ProxyTerminalReason, UserId, deterministic_uuid,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, postgres::PgRow};
use uuid::Uuid;

use crate::database::audit_health::AuditWriteResult as _;

use super::PostgresStore;
#[path = "postgres/admin.rs"]
mod admin;
mod bid_queue;
#[path = "postgres/bidding/mod.rs"]
mod bidding;
#[path = "postgres/lifecycle.rs"]
mod lifecycle;
#[path = "postgres/mapping.rs"]
mod mapping;

pub(crate) use mapping::*;

pub(crate) use bid_queue::AuctionBidQueue;
#[cfg(feature = "test-db")]
pub use bid_queue::BidQueueMode;

type PgTransaction = Transaction<'static, Postgres>;

/// PostgreSQL 竞价事务的唯一实现；事务对象拥有 SQLx 连接，回调结束后 runner 仍能提交或回滚。
/// 回调不能自取独立连接，也不能把命令账本和竞价写入拆成两个事务。
pub struct PostgresCompetitionTransaction {
    transaction: Option<PgTransaction>,
    /// W12.1 已写入待定序号事件（`stream_sequence IS NULL`）的 stream key；提交后补号。
    /// 同一命令的多条竞价事实属于同一场次，因此只在 key 变化时追加。
    deferred_stream_keys: Vec<String>,
    /// W12.1 残留：本事务追加的标记事件；提交后由独立事务应用标记与提醒，回滚则一并丢弃。
    lot_watch_events: Vec<Uuid>,
    /// 锁内快照同时读出的场次 ID。实时事件与竞价事实同事务写入时不必再次读取 `lots`：
    /// 额外的等值读取会在 Serializable 下叠加谓词锁（W12.1 残留的跨 lot 误报来源）。
    loaded_auction_id: Option<Uuid>,
}

impl PostgresCompetitionTransaction {
    pub(crate) fn new(transaction: PgTransaction) -> Self {
        Self {
            transaction: Some(transaction),
            deferred_stream_keys: Vec::new(),
            lot_watch_events: Vec::new(),
            loaded_auction_id: None,
        }
    }

    /// 记录本事务写的待定 stream；提交后由调用方在同一进程内补号（失败由 relay 兜底）。
    pub(crate) fn record_deferred_stream(&mut self, stream_key: &str) {
        if self.deferred_stream_keys.last().map(String::as_str) != Some(stream_key) {
            self.deferred_stream_keys.push(stream_key.to_owned());
        }
    }

    /// 取出待补号的 stream key；提交成功后才允许分配，回滚则一并丢弃。
    pub(crate) fn take_deferred_stream_keys(&mut self) -> Vec<String> {
        std::mem::take(&mut self.deferred_stream_keys)
    }

    /// 记录本事务追加的标记事件；提交后立即应用，回滚时随事务丢弃。
    pub(crate) fn record_lot_watch_event(&mut self, event_id: Uuid) {
        self.lot_watch_events.push(event_id);
    }

    /// 取出待处理事件 ID；只有提交成功后才允许处理。
    pub(crate) fn take_lot_watch_events(&mut self) -> Vec<Uuid> {
        std::mem::take(&mut self.lot_watch_events)
    }

    /// 锁内快照读出的场次 ID；调用方据此写实时事件，避免再次探测 `lots`。
    pub(crate) fn loaded_auction_id(&self) -> Option<Uuid> {
        self.loaded_auction_id
    }

    pub(crate) fn record_loaded_auction(&mut self, auction_id: Uuid) {
        self.loaded_auction_id = Some(auction_id);
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
        self.pool().observe_transaction(self.transaction_owner, async move {
            if let Err(error) = TransactionPolicy::new(policy.isolation, policy.max_attempts) {
                return Err(E::from(error));
            }

            for attempt in 0..policy.max_attempts {
                let transaction =
                    match self.pool().begin_with(begin_statement(policy.isolation)).await {
                        Ok(transaction) => transaction,
                        Err(error) => return Err(E::from(map_sql_error(error))),
                    };
                let mut transaction = PostgresCompetitionTransaction::new(transaction);

                match operation(&mut transaction).await {
                    Ok(value) => {
                        // W12.1：竞价事件的序号在提交后分配，回滚时随事务一起丢弃。
                        let deferred = transaction.take_deferred_stream_keys();
                        let lot_watch_events = transaction.take_lot_watch_events();
                        match transaction.commit().await {
                            Ok(()) => {
                                crate::database::realtime::allocate_deferred_sequences_after_commit(
                                    self, &deferred,
                                )
                                .await;
                                // W12.1 残留：标记与提醒在提交后的独立事务应用；失败不回滚出价，
                                // 持久事件仍由 Worker 扫描重试。
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
                                    CompetitionTransactionError::SerializationConflict
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
                                        CompetitionTransactionError::SerializationConflict
                                    ),
                                );
                                return Err(E::from(error));
                            }
                        }
                    }
                    Err(error) => {
                        if error.is_serialization_conflict() && attempt + 1 < policy.max_attempts {
                            crate::database::serializable_retry_backoff(usize::from(attempt + 1))
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

            Err(E::from(CompetitionTransactionError::SerializationConflict))
        })
    }

    /// 同一拍卖的可写竞价命令：先在队列上取得凭证，再按既有策略开启 Serializable 事务。
    ///
    /// 排队发生在观察窗口之外——记录的事务耗时只覆盖真正的尝试；凭证一直持有到事务、提交后
    /// 补号与标记事件处理结束，因此同一场次的下一条命令看到的是完整结果。取消或错误都会经
    /// `Drop` 释放凭证（进程内凭证与会话 advisory lock），不会留下无人持有的锁。
    #[allow(clippy::manual_async_fn)]
    fn run_in_auction_bid_queue<T, E, F>(
        &self,
        lot_id: LotId,
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: TransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send,
    {
        async move {
            let lease = self
                .auction_bid_queue()
                .enqueue(self.pool(), lot_id.as_uuid())
                .await
                .map_err(E::from)?;
            let result = self.run(policy, operation).await;
            lease.release().await;
            result
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
    ) -> impl Future<Output = Result<CommandInsert, CompetitionTransactionError>> + Send {
        async move { self.insert_command_inner(&command).await }
    }

    fn current_time(
        &mut self,
    ) -> impl Future<Output = Result<DateTime<Utc>, CompetitionTransactionError>> + Send {
        async move { self.current_time_inner().await }
    }

    fn load_for_update(
        &mut self,
        lot_id: LotId,
    ) -> impl Future<Output = Result<CompetitionSnapshot, CompetitionTransactionError>> + Send {
        async move { self.load_for_update_inner(lot_id).await }
    }

    fn participant_is_eligible_for_update(
        &mut self,
        user_id: UserId,
    ) -> impl Future<Output = Result<bool, CompetitionTransactionError>> + Send {
        async move { self.participant_is_eligible_for_update_inner(user_id).await }
    }

    fn proxy_usage_context(
        &mut self,
        lot_id: LotId,
        user_id: UserId,
    ) -> impl Future<Output = Result<ProxyUsageContext, CompetitionTransactionError>> + Send {
        async move { self.proxy_usage_context_inner(lot_id, user_id).await }
    }

    fn proxy_transition_history(
        &mut self,
        authorization_id: ProxyAuthorizationId,
    ) -> impl Future<
        Output = Result<Vec<CompetitionProxyTransitionRecord>, CompetitionTransactionError>,
    > + Send {
        async move { self.proxy_transition_history_inner(authorization_id).await }
    }

    fn proxy_quota_limit(
        &mut self,
        lot_id: LotId,
    ) -> impl Future<Output = Result<u32, CompetitionTransactionError>> + Send {
        async move { self.proxy_quota_limit_inner(lot_id).await }
    }

    fn apply_plan(
        &mut self,
        plan: &CompetitionPlan,
        audit: Option<CompetitionCommandAuditContext>,
        occurred_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { self.apply_public_command_plan_inner(plan, audit, occurred_at).await }
    }

    fn apply_slow_proxy_plan(
        &mut self,
        plan: &CompetitionPlan,
        context: SlowProxyExecutionContext,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { self.apply_slow_proxy_plan_inner(plan, context).await }
    }
}
