//! 结算履约 prepared delivery 写入与 activation 推进。

use super::mapping::*;

use chrono::Utc;
use sqlx::Postgres;

use std::{future::Future, pin::Pin};

mod activate;
mod prepare;

pub(super) use activate::activate_postgres;
pub(super) use prepare::prepare_postgres;

type DeliveryTransactionFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, DeliveryTransactionError>> + Send + 'a>>;

pub(super) enum DeliveryTransactionError {
    Sqlx(sqlx::Error),
    Repository(auction_application::core::RepositoryError),
    Funding(auction_application::settlement::FundingError),
}

impl From<sqlx::Error> for DeliveryTransactionError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}

impl From<auction_application::core::RepositoryError> for DeliveryTransactionError {
    fn from(error: auction_application::core::RepositoryError) -> Self {
        Self::Repository(error)
    }
}

impl DeliveryTransactionError {
    fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Funding(auction_application::settlement::FundingError::SerializationConflict)
        ) || matches!(self, Self::Sqlx(error)
                if crate::database::observed_serialization_conflict(error))
    }

    fn into_repository_error(self) -> auction_application::core::RepositoryError {
        match self {
            Self::Sqlx(error) => map_storage_error(error),
            Self::Repository(error) => error,
            Self::Funding(_) => repository_error("履约业务锁或资金保护门不可用"),
        }
    }
}

/// 按旧版 `RunSerializable(..., 3)` 语义执行履约 owner 事务。
/// 只有 PostgreSQL 明确报告的序列化冲突（40001）或死锁（40P01）才重试；业务身份冲突、claim 失效
/// 和其他数据库错误直接返回。每次重试从头开启事务，但调用方传入的 delivery 与 audit 身份不变。
pub(super) async fn run_serializable_delivery_transaction<T, F>(
    pool: &crate::database::TrackedPool<Postgres>,
    custody: Option<(
        &auction_application::settlement::SettlementInternalEvidenceSnapshot,
        std::time::Instant,
    )>,
    mut operation: F,
) -> Result<T, auction_application::core::RepositoryError>
where
    T: Send,
    F: for<'a> FnMut(
            &'a mut crate::database::TrackedTransaction<'static, Postgres>,
            Option<&'a auction_application::settlement::SettlementInternalEvidenceSnapshot>,
        ) -> DeliveryTransactionFuture<'a, T>
        + Send,
{
    pool.observe_transaction(auction_observability::DatabaseTransactionOwner::Fulfillment, async {
        let deadline = custody.map(|(_, deadline)| deadline);
        let custody = custody.map(|(snapshot, _)| snapshot);
        let run = async {
            for attempt in 0..3 {
                let mut transaction =
                    match pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await {
                        Ok(transaction) => transaction,
                        Err(error) => {
                            let error = DeliveryTransactionError::from(error);
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
                    };

                let operation_result = async {
                    if let Some(custody) = custody {
                        let budget = (custody.expires_at - Utc::now()).num_milliseconds();
                        if budget <= 0 {
                            return Err(DeliveryTransactionError::Funding(
                                auction_application::settlement::FundingError::Unproven,
                            ));
                        }
                        // PostgreSQL 18 整事务预算包含锁等待与 COMMIT，重试不延长证据窗口。
                        sqlx::query("SELECT set_config('transaction_timeout',$1,true)")
                            .bind(format!("{budget}ms"))
                            .execute(&mut *transaction)
                            .await?;
                    }
                    let value = operation(&mut transaction, custody).await?;
                    if let Some(custody) = custody {
                        crate::validate_mail_evidence_time(&mut transaction, custody)
                            .await
                            .map_err(super::internal::evidence_error)
                            .map_err(DeliveryTransactionError::Funding)?;
                    }
                    Ok(value)
                }
                .await;
                match operation_result {
                    Ok(value) => match transaction.commit().await {
                        Ok(()) => return Ok(value),
                        Err(error) => {
                            let error = DeliveryTransactionError::from(error);
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
                        if retryable && attempt < 2 {
                            let _ = transaction.rollback().await;
                            continue;
                        }
                        let _ = transaction.rollback().await;
                        crate::database::observe_retry_exhaustion(attempt + 1, 3, retryable);
                        return Err(mapped);
                    }
                }
            }

            Err(repository_error("Serializable 履约事务重试状态异常"))
        };
        if let Some(deadline) = deadline {
            // 外层覆盖连接池排队；取消或超时由调用方尚未确认的 guard 撤销当前 owner。
            tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), run)
                .await
                .map_err(|_| repository_error("退款库存证据提交预算已耗尽"))?
        } else {
            run.await
        }
    })
    .await
}
