use std::{future::Future, pin::Pin};

use thiserror::Error;

use auction_domain::{
    AuctionRules, CompetitionPlan, LotBidState, LotId, ProxyAuthorization, UserId,
};

use super::core_ledger::{CommandLedgerError, StoredCommand};
use super::core_policy::{TransactionPolicy, TransactionPolicyError};

/// 竞价命令在锁内读取到的最小聚合快照。
#[derive(Debug, Clone)]
pub struct CompetitionSnapshot {
    pub rules: AuctionRules,
    pub state: LotBidState,
    pub active_proxy: Option<ProxyAuthorization>,
}

#[derive(Debug, Error)]
pub enum CompetitionTransactionError {
    #[error("竞价投影不存在")]
    NotFound,
    #[error("竞价事务中的资源状态发生冲突")]
    Conflict,
    #[error("管理 lot 的状态版本已经过期")]
    StaleState,
    #[error("管理 lot 当前状态不允许该命令")]
    InvalidState,
    #[error("竞价投影读取或写入失败: {0}")]
    Failed(String),
    #[error("竞价事务发生可重试的序列化冲突")]
    SerializationConflict,
    #[error(transparent)]
    CommandLedger(#[from] CommandLedgerError),
}

/// 允许事务 runner 判断一次应用失败是否应按事务策略重试。
pub trait TransactionFailure:
    From<CompetitionTransactionError> + From<TransactionPolicyError> + Send
{
    fn is_serialization_conflict(&self) -> bool;
}

/// 事务回调的异步返回类型，使 runner 能在提交前继续持有事务对象。
pub type TransactionFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

/// 竞价事务对象。实现必须在同一数据库事务中锁定快照并原子应用计划。
pub trait CompetitionTransaction: Send {
    fn find_command(
        &mut self,
        command_id: auction_domain::CommandId,
    ) -> impl Future<Output = Result<Option<StoredCommand>, CompetitionTransactionError>> + Send;

    fn insert_command(
        &mut self,
        command: StoredCommand,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;

    fn load_for_update(
        &mut self,
        lot_id: LotId,
    ) -> impl Future<Output = Result<CompetitionSnapshot, CompetitionTransactionError>> + Send;

    /// 在已锁定 lot 的事务中计算该用户下一次代理授权的使用次数。
    fn next_proxy_usage_number(
        &mut self,
        lot_id: LotId,
        user_id: UserId,
    ) -> impl Future<Output = Result<u64, CompetitionTransactionError>> + Send;

    fn apply_plan(
        &mut self,
        plan: CompetitionPlan,
        request_id: Option<String>,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;
}

/// 事务 runner 端口。实现负责开始、提交/回滚和后端隔离级别映射。
pub trait TransactionRunner: Send + Sync {
    type Transaction: CompetitionTransaction;

    fn run<T, E, F>(
        &self,
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: TransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send;
}
