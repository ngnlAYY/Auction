//! Delivery Admin PostgreSQL SERIALIZABLE 事务边界与错误分类。

use std::time::Duration;

use auction_application::DeliveryAdminError;
use sqlx::{PgConnection, Postgres, Transaction};

use crate::database::TrackedPool;

/// Delivery Admin 单次事务中的错误；SQLx 原始错误必须保留到重试边界。
pub(super) enum DeliveryAdminTransactionError {
    /// 数据库驱动错误，只有明确的串行化冲突才允许重试。
    Sqlx(sqlx::Error),
    /// 已经确定的业务结果，不得因为事务重放而改变分类。
    Business(DeliveryAdminError),
    /// 保留资金owner的类型化冲突，直到同一外层runner决定是否重试。
    Funding(auction_application::FundingError),
}

impl From<DeliveryAdminError> for DeliveryAdminTransactionError {
    fn from(error: DeliveryAdminError) -> Self {
        Self::Business(error)
    }
}

impl DeliveryAdminTransactionError {
    /// 判断 SQLSTATE 是否属于旧版 `RunSerializable` 的重试闭集。
    pub(super) fn is_retryable(&self) -> bool {
        if matches!(self, Self::Funding(auction_application::FundingError::SerializationConflict)) {
            return true;
        }
        let Self::Sqlx(error) = self else {
            return false;
        };
        crate::database::is_serialization_conflict(error)
    }

    /// 在离开存储层时只暴露稳定的 Delivery Admin 错误分类。
    pub(super) fn into_delivery_error(self) -> DeliveryAdminError {
        match self {
            Self::Sqlx(_) | Self::Funding(_) => DeliveryAdminError::Unavailable,
            Self::Business(error) => error,
        }
    }
}

/// 一个已设置为 SERIALIZABLE 的完整 PostgreSQL 事务。
pub(super) struct PostgresDeliveryAdminTransaction {
    transaction: Option<Transaction<'static, Postgres>>,
}

impl PostgresDeliveryAdminTransaction {
    /// 从连接池开始一次全新的 SERIALIZABLE 事务；隔离级别随 BEGIN 声明，失败保留原始错误。
    pub(super) async fn begin(
        pool: &TrackedPool<Postgres>,
    ) -> Result<Self, DeliveryAdminTransactionError> {
        let transaction = pool
            .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
            .await
            .map_err(DeliveryAdminTransactionError::Sqlx)?;
        Ok(Self { transaction: Some(transaction) })
    }

    /// 返回底层事务，供同一事务的审计写入使用。
    pub(super) fn transaction_mut(
        &mut self,
    ) -> Result<&mut Transaction<'static, Postgres>, DeliveryAdminTransactionError> {
        self.transaction
            .as_mut()
            .ok_or(DeliveryAdminTransactionError::Business(DeliveryAdminError::Unavailable))
    }

    /// 返回底层连接，供 SQLx 查询使用；不会创建第二个事务。
    pub(super) fn connection(
        &mut self,
    ) -> Result<&mut PgConnection, DeliveryAdminTransactionError> {
        Ok(&mut **self.transaction_mut()?)
    }

    /// 提交完整事务；commit 的 SQLx source 继续交给最外层重试分类。
    pub(super) async fn commit(mut self) -> Result<(), DeliveryAdminTransactionError> {
        match self.transaction.take() {
            Some(transaction) => {
                transaction.commit().await.map_err(DeliveryAdminTransactionError::Sqlx)
            }
            None => Ok(()),
        }
    }

    /// 回滚竞态事务；回滚失败同样不能丢掉 SQLx source。
    pub(super) async fn rollback(mut self) -> Result<(), DeliveryAdminTransactionError> {
        match self.transaction.take() {
            Some(transaction) => {
                transaction.rollback().await.map_err(DeliveryAdminTransactionError::Sqlx)
            }
            None => Ok(()),
        }
    }
}

/// 返回与旧版一致的两段重试等待：第一次 10ms，第二次 20ms。
pub(super) async fn wait_before_retry(attempt: usize) {
    tokio::time::sleep(Duration::from_millis(10 * (attempt as u64 + 1))).await;
}
