//! Recovery action 的生命周期模块装配；领取、投递推进和终态收口位于子模块，统一经
//! `RecoveryWorkStore` 暴露，避免把外部副作用带入存储层。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
use std::{future::Future, pin::Pin};

#[cfg(feature = "postgres")]
type RecoveryActionTransactionFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, RecoveryActionTransactionError>> + Send + 'a>>;

#[cfg(feature = "postgres")]
pub(super) enum RecoveryActionTransactionError {
    Storage(sqlx::Error),
    Repository(auction_application::RepositoryError),
}

#[cfg(feature = "postgres")]
impl From<sqlx::Error> for RecoveryActionTransactionError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(error)
    }
}

#[cfg(feature = "postgres")]
impl From<auction_application::RepositoryError> for RecoveryActionTransactionError {
    fn from(error: auction_application::RepositoryError) -> Self {
        Self::Repository(error)
    }
}

#[cfg(feature = "postgres")]
impl RecoveryActionTransactionError {
    fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Storage(sqlx::Error::Database(error))
                if matches!(error.code().as_deref(), Some("40001" | "40P01"))
        )
    }

    fn into_repository_error(self) -> auction_application::RepositoryError {
        match self {
            Self::Storage(error) => map_storage_error(error),
            Self::Repository(error) => error,
        }
    }
}

/// 按旧版 `RunSerializable(..., 3)` 语义执行 recovery action owner 事务。
/// 仅重试 PostgreSQL 序列化冲突与死锁；claim fence、业务身份冲突和其他数据库错误立即失败。调用方
/// 必须在重试轮次外生成 command、claim 或 audit 身份，确保一次逻辑提交身份稳定。
#[cfg(feature = "postgres")]
pub(super) async fn run_serializable_action_transaction<T, F>(
    pool: &crate::database::TrackedPool<Postgres>,
    mut operation: F,
) -> Result<T, auction_application::RepositoryError>
where
    T: Send,
    F: for<'a> FnMut(
            &'a mut sqlx::Transaction<'static, Postgres>,
        ) -> RecoveryActionTransactionFuture<'a, T>
        + Send,
{
    for attempt in 0..3 {
        let mut transaction = match pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await {
            Ok(transaction) => transaction,
            Err(error) => {
                let error = RecoveryActionTransactionError::from(error);
                if error.is_retryable() && attempt < 2 {
                    continue;
                }
                return Err(error.into_repository_error());
            }
        };

        match operation(&mut transaction).await {
            Ok(value) => match transaction.commit().await {
                Ok(()) => return Ok(value),
                Err(error) => {
                    let error = RecoveryActionTransactionError::from(error);
                    if error.is_retryable() && attempt < 2 {
                        continue;
                    }
                    return Err(error.into_repository_error());
                }
            },
            Err(error) => {
                let retryable = error.is_retryable();
                let mapped = error.into_repository_error();
                let _ = transaction.rollback().await;
                if retryable && attempt < 2 {
                    continue;
                }
                return Err(mapped);
            }
        }
    }

    Err(repository_error("recovery action Serializable 重试状态异常"))
}

/// 先锁 action，再由后续 UPDATE 检查实时租约；需要父 case 的调用方必须先锁 recovery。
#[cfg(feature = "postgres")]
async fn lock_action(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    action_id: Uuid,
) -> Result<(), RecoveryActionTransactionError> {
    sqlx::query("SELECT id FROM public.settlement_recovery_actions WHERE id = $1 FOR UPDATE")
        .bind(action_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(RecoveryActionTransactionError::from)?;
    Ok(())
}

#[cfg(feature = "postgres")]
#[path = "actions/claim.rs"]
mod claim;
#[cfg(feature = "postgres")]
#[path = "actions/delivery/mod.rs"]
mod delivery;
#[cfg(feature = "postgres")]
#[path = "actions/resolution.rs"]
mod resolution;

#[cfg(feature = "postgres")]
pub(super) use claim::claim_action_postgres;
#[cfg(feature = "postgres")]
pub(super) use delivery::{
    link_action_postgres, mark_action_enqueued_postgres, observe_action_postgres,
};
#[cfg(feature = "postgres")]
pub(super) use resolution::{
    manual_action_postgres, mark_action_satisfied_postgres, release_action_postgres,
};
