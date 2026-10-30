use std::future::Future;

use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

use auction_domain::{
    AuctionId, CommandId, CompetitionPlan, RecoveryPolicy, SettlementAmount, SettlementFact,
    SettlementLine, SettlementMode, SettlementObligationKind, SettlementPlan, SettlementPolicy,
};

use super::{
    CommandLedgerError, CompetitionSnapshot, StoredCommand, TransactionFuture, TransactionPolicy,
    TransactionPolicyError,
};

/// 已结算拍卖中按义务种类聚合的权威数量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedSettlementObligationCount {
    pub kind: SettlementObligationKind,
    pub count: u64,
}

/// 已结算拍卖的权威持久化事实，用于兼容接管旧版命令账本的只读重放。
/// 该投影只包含结算摘要、不可变 lot 金额和义务种类数量；履约阶段会改变的义务状态不属于
/// 结算命令原始结果，不能拿当前履约状态覆盖历史重放。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedSettlement {
    pub command_id: CommandId,
    /// workflow 完成 fence；旧版在结算事务内写入，新版可能由下一步 completion-only 补写。
    pub workflow_complete_command_id: Option<CommandId>,
    pub policy_version: u32,
    pub policy_mode: SettlementMode,
    pub pool_enabled: bool,
    pub opening_pool: SettlementAmount,
    pub closing_pool: SettlementAmount,
    pub opening_pool_version: u64,
    pub closing_pool_version: u64,
    pub tax_total: SettlementAmount,
    pub discount_demand: SettlementAmount,
    pub compensation_total: SettlementAmount,
    pub sold_count: u64,
    pub unsold_count: u64,
    pub obligation_counts: Vec<PersistedSettlementObligationCount>,
    pub state_version: u64,
    pub settled_at: DateTime<Utc>,
    pub lines: Vec<SettlementLine>,
}

/// 结算事务锁内读取的拍卖事实。
/// `facts` 必须来自同一事务锁定的拍卖和 lot；应用层不接受由 HTTP 或 Worker 自行拼接的
/// 部分快照，否则结算明细与状态版本可能来自不同时间点。
#[derive(Debug, Clone)]
pub struct SettlementSnapshot {
    pub auction_id: AuctionId,
    pub auction_state_version: u64,
    /// 锁内冻结的结算工作流身份；历史已结算记录可能缺少此字段。
    pub workflow_id: Option<Uuid>,
    /// 锁内领取命令身份；首次结算必须与调用方一致。
    pub claim_command_id: Option<CommandId>,
    /// 锁内冻结的结算截止世代；首次结算必须与调用方一致。
    pub due_at: Option<DateTime<Utc>>,
    pub policy: SettlementPolicy,
    /// 结算时一并冻结的 recovery 策略；后续 Worker 不能读取实时规则替换它。
    pub recovery_policy: RecoveryPolicy,
    pub scheduled_open_at: DateTime<Utc>,
    pub scheduled_end_at: DateTime<Utc>,
    pub opening_pool: SettlementAmount,
    pub opening_pool_version: u64,
    pub facts: Vec<SettlementFact>,
    /// 首次结算时用于领域层关闭活动代理的锁内竞价快照；已结算命令重放时为空。
    pub lot_competitions: Vec<CompetitionSnapshot>,
    /// 仅在拍卖已经 settled 时存在；用于把旧版摘要账本与当前权威事实逐项对账。
    pub persisted_settlement: Option<PersistedSettlement>,
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
        occurred_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), SettlementTransactionError>> + Send;

    /// 借用已校验计划写入同一事务；用例保留原计划作为结果，不复制完整明细与义务。
    fn apply_settlement(
        &mut self,
        auction_id: AuctionId,
        command_id: auction_domain::CommandId,
        plan: &SettlementPlan,
        proxy_closure_plans: Vec<CompetitionPlan>,
        recovery_policy: RecoveryPolicy,
        occurred_at: DateTime<Utc>,
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
