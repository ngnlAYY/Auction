//! 标记命令、完整集合及事务窄端口；会话身份由 HTTP 装配，不接受客户端 user_id。

use std::{future::Future, pin::Pin};

use auction_domain::CommandId;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    CompetitionTransactionError, StoredCommand, TransactionFailure, TransactionPolicyError,
};

/// 完整集合的防御上限；超限不得截断并伪装未标记。
pub const MAX_LOT_WATCHES: usize = 10_000;

/// 单项持久状态，版本零仅表示不存在，不作为持久记录返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LotWatch {
    pub lot_id: Uuid,
    pub watched: bool,
    pub channel: String,
    pub version: i64,
}

/// 标记命令的可重放结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetLotWatchResult {
    pub user_id: Uuid,
    pub auction_id: Uuid,
    pub watch: LotWatch,
}

/// 稳定命令负载；用户身份纳入摘要以阻止跨账号重放。
#[derive(Debug, Clone, Serialize)]
pub struct SetLotWatchCommand {
    pub command_id: CommandId,
    pub user_id: Uuid,
    pub lot_id: Uuid,
    pub watched: bool,
    pub channel: String,
    pub expected_version: i64,
}

/// 标记边界错误，持久错误不包含 SQL 或用户敏感值。
#[derive(Debug, Error)]
pub enum LotWatchError {
    #[error("标记输入无效或渠道未启用")]
    Invalid,
    #[error("标记命令或版本冲突")]
    Conflict,
    #[error("公开拍卖或标的不存在")]
    NotFound,
    #[error("标记依赖暂不可用")]
    Unavailable,
    #[error("标记集合超过完整读取上限")]
    TooMany,
    #[error(transparent)]
    Transaction(#[from] CompetitionTransactionError),
}

impl From<TransactionPolicyError> for LotWatchError {
    fn from(_: TransactionPolicyError) -> Self {
        Self::Unavailable
    }
}

impl TransactionFailure for LotWatchError {
    fn is_serialization_conflict(&self) -> bool {
        matches!(self, Self::Transaction(CompetitionTransactionError::SerializationConflict))
    }
}

/// 标记异步端口结果。
pub type LotWatchFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, LotWatchError>> + Send + 'a>>;

/// Public 标记用例入口；只读取指定用户，不枚举其他用户。
pub trait LotWatchPort: Send + Sync {
    fn list(&self, user_id: Uuid, auction_id: Uuid) -> LotWatchFuture<'_, Vec<LotWatch>>;
    fn set(&self, command: SetLotWatchCommand) -> LotWatchFuture<'_, SetLotWatchResult>;
}

/// 同一 Serializable 事务中锁定 world → auction → lot，再执行账本与 CAS。
pub trait LotWatchTransaction: Send {
    fn lock_watch_lot(
        &mut self,
        lot_id: Uuid,
    ) -> impl Future<Output = Result<Uuid, LotWatchError>> + Send;
    fn find_watch_command(
        &mut self,
        command_id: CommandId,
    ) -> impl Future<Output = Result<Option<StoredCommand>, LotWatchError>> + Send;
    fn write_watch(
        &mut self,
        command: &SetLotWatchCommand,
    ) -> impl Future<Output = Result<LotWatch, LotWatchError>> + Send;
    fn store_watch_command(
        &mut self,
        command: StoredCommand,
    ) -> impl Future<Output = Result<(), LotWatchError>> + Send;
}
