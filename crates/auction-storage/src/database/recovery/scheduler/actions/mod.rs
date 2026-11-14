//! Recovery action 的生命周期模块装配；领取、投递推进和终态收口位于子模块，统一经
//! `RecoveryWorkStore` 暴露，避免把外部副作用带入存储层。

use std::{future::Future, pin::Pin};

use sqlx::Postgres;
use uuid::Uuid;

use super::mapping::map_storage_error;
use crate::database::recovery::scheduler::repository_error;

type RecoveryActionTransactionFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, RecoveryActionTransactionError>> + Send + 'a>>;

pub(super) enum RecoveryActionTransactionError {
    Storage(sqlx::Error),
    Repository(auction_application::core::RepositoryError),
}

impl From<sqlx::Error> for RecoveryActionTransactionError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(error)
    }
}

impl From<auction_application::core::RepositoryError> for RecoveryActionTransactionError {
    fn from(error: auction_application::core::RepositoryError) -> Self {
        Self::Repository(error)
    }
}

impl RecoveryActionTransactionError {
    fn is_retryable(&self) -> bool {
        matches!(self, Self::Storage(error)
            if crate::database::observed_serialization_conflict(error))
    }

    fn into_repository_error(self) -> auction_application::core::RepositoryError {
        match self {
            Self::Storage(error) => map_storage_error(error),
            Self::Repository(error) => error,
        }
    }
}

/// 按旧版 `RunSerializable(..., 3)` 语义执行 recovery action owner 事务。
/// 仅重试 PostgreSQL 序列化冲突与死锁；claim fence、业务身份冲突和其他数据库错误立即失败。调用方
/// 必须在重试轮次外生成 command、claim 或 audit 身份，确保一次逻辑提交身份稳定。
pub(super) async fn run_serializable_action_transaction<T, F>(
    pool: &crate::database::TrackedPool<Postgres>,
    mut operation: F,
) -> Result<T, auction_application::core::RepositoryError>
where
    T: Send,
    F: for<'a> FnMut(
            &'a mut crate::database::TrackedTransaction<'static, Postgres>,
        ) -> RecoveryActionTransactionFuture<'a, T>
        + Send,
{
    pool.observe_transaction(auction_observability::DatabaseTransactionOwner::Recovery, async {
        for attempt in 0..3 {
            let mut transaction = match pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await
            {
                Ok(transaction) => transaction,
                Err(error) => {
                    let error = RecoveryActionTransactionError::from(error);
                    if error.is_retryable() && attempt < 2 {
                        continue;
                    }
                    crate::database::observe_retry_exhaustion(attempt + 1, 3, error.is_retryable());
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
                        crate::database::observe_retry_exhaustion(
                            attempt + 1,
                            3,
                            error.is_retryable(),
                        );
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
                    crate::database::observe_retry_exhaustion(attempt + 1, 3, retryable);
                    return Err(mapped);
                }
            }
        }

        Err(repository_error("recovery action Serializable 重试状态异常"))
    })
    .await
}

/// 先锁 action，再由后续 UPDATE 检查实时租约；需要父 case 的调用方必须先锁 recovery。
async fn lock_action(
    transaction: &mut sqlx::PgConnection,
    action_id: Uuid,
) -> Result<(), RecoveryActionTransactionError> {
    sqlx::query("SELECT id FROM public.settlement_recovery_actions WHERE id = $1 FOR UPDATE")
        .bind(action_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(RecoveryActionTransactionError::from)?;
    Ok(())
}

mod claim;
mod delivery;
mod resolution;

pub(super) use claim::claim_action_postgres;
pub(super) use delivery::{
    link_action_postgres, mark_action_enqueued_postgres, observe_action_postgres,
};
pub(super) use resolution::{
    manual_action_postgres, mark_action_satisfied_postgres, release_action_postgres,
};
