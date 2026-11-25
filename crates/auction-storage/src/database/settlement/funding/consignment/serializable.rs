//! 收款侧入口共用的顶层 Serializable 事务循环。
//!
//! 只对已确认回滚的序列化冲突以原输入有界重试（不变量 2）；延迟约束在 COMMIT 时失败属于确定回滚，
//! 按约束名分类，其余提交错误按存储失败交由调用方用原命令 ID 核对（不变量 1）。

use std::future::Future;
use std::pin::Pin;

use auction_application::settlement::FundingError;
use sqlx::Postgres;

use super::receipt::funding_sql_error;
use crate::DatabaseStore;
use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::storage;

/// 事务体的装箱 future；借用事务直到事务体结束。
pub(super) type TxFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, FundingError>> + Send + 'a>>;

/// 每次尝试都开启新的 Serializable 事务并以原命令整体重跑 `body`；`body` 不得自行提交或回滚。
pub(super) async fn run_serializable<C, T, F>(
    store: &DatabaseStore,
    command: &C,
    body: F,
) -> Result<T, FundingError>
where
    F: for<'a, 'c> Fn(&'a mut Transaction<'c, Postgres>, &'a C) -> TxFuture<'a, T>,
{
    crate::database::retry_conflicts(
        store.pool(),
        auction_observability::DatabaseTransactionOwner::Funding,
        crate::database::SERIALIZABLE_ATTEMPTS,
        |error: &FundingError| matches!(error, FundingError::SerializationConflict),
        || attempt(store, command, &body),
    )
    .await
}

async fn attempt<C, T, F>(store: &DatabaseStore, command: &C, body: &F) -> Result<T, FundingError>
where
    F: for<'a, 'c> Fn(&'a mut Transaction<'c, Postgres>, &'a C) -> TxFuture<'a, T>,
{
    let mut tx =
        store.pool().begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await.map_err(storage)?;
    match body(&mut tx, command).await {
        Ok(value) => {
            tx.commit().await.map_err(funding_sql_error)?;
            Ok(value)
        }
        Err(error) => {
            // 只有显式 ROLLBACK 已确认才允许释放连接并重试。
            tx.rollback().await.map_err(storage)?;
            Err(error)
        }
    }
}
