use std::future::Future;

use chrono::{DateTime, Utc};
use thiserror::Error;

use auction_domain::{
    AuctionId, SettlementAmount, SettlementFact, SettlementPlan, SettlementPolicy,
};

use super::{
    CommandLedgerError, StoredCommand, TransactionFuture, TransactionPolicy, TransactionPolicyError,
};

/// 结算事务锁内读取的拍卖事实。
///
/// `facts` 必须来自同一事务锁定的拍卖和 lot；应用层不接受由 HTTP 或 Worker 自行拼接的
/// 部分快照，否则结算明细与状态版本可能来自不同时间点。
#[derive(Debug, Clone)]
pub struct SettlementSnapshot {
    pub auction_id: AuctionId,
    pub auction_state_version: u64,
    pub policy: SettlementPolicy,
    pub scheduled_open_at: DateTime<Utc>,
    pub scheduled_end_at: DateTime<Utc>,
    pub opening_pool: SettlementAmount,
    pub opening_pool_version: u64,
    pub facts: Vec<SettlementFact>,
}

#[derive(Debug, Error)]
pub enum SettlementTransactionError {
    #[error("结算拍卖不存在或不在可结算状态")]
    NotFound,
    #[error("结算事务中的资源状态发生冲突")]
    Conflict,
    #[error("结算事务读取或写入失败: {0}")]
    Failed(String),
    #[error("结算事务发生可重试的序列化冲突")]
    SerializationConflict,
    #[error(transparent)]
    CommandLedger(#[from] CommandLedgerError),
}

/// 结算 runner 使用的失败类型边界，与竞价 runner 的错误集合保持分离。
pub trait SettlementTransactionFailure:
    From<SettlementTransactionError> + From<TransactionPolicyError> + Send
{
    fn is_serialization_conflict(&self) -> bool {
        false
    }
}

/// 结算事务对象。实现必须锁定拍卖、lot 和结算池，并在同一事务写入所有结算事实。
pub trait SettlementTransaction: Send {
    fn find_command(
        &mut self,
        command_id: auction_domain::CommandId,
    ) -> impl Future<Output = Result<Option<StoredCommand>, SettlementTransactionError>> + Send;

    fn load_for_update(
        &mut self,
        auction_id: AuctionId,
    ) -> impl Future<Output = Result<SettlementSnapshot, SettlementTransactionError>> + Send;

    fn insert_command(
        &mut self,
        command: StoredCommand,
    ) -> impl Future<Output = Result<(), SettlementTransactionError>> + Send;

    fn apply_settlement(
        &mut self,
        auction_id: AuctionId,
        command_id: auction_domain::CommandId,
        plan: SettlementPlan,
    ) -> impl Future<Output = Result<(), SettlementTransactionError>> + Send;
}

/// 结算事务 runner。它与竞价 runner 分开，防止把不同锁集合和提交步骤误用在一起。
pub trait SettlementTransactionRunner: Send + Sync {
    type Transaction: SettlementTransaction;

    fn run<T, E, F>(
        &self,
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: SettlementTransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send;
}
