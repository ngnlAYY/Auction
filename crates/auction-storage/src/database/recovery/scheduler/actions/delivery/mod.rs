//! Recovery action 投递状态推进与结果观察的模块装配；pending → prepared → enqueued 的身份推进和
//! 终态证明由子模块负责，本模块保留稳定出口与原生事务借用适配。

use sqlx::Postgres;

use crate::database::recovery::scheduler::actions::RecoveryActionTransactionFuture;

pub(in crate::database::recovery::scheduler) mod activation;
pub(in crate::database::recovery::scheduler) mod observation;
pub(in crate::database::recovery::scheduler) mod prepare;

/// 原生观察回调仍借用外层受跟踪的同一事务，不另取连接或复制重试循环。
async fn run_serializable_action_transaction<T, F>(
    pool: &crate::database::TrackedPool<Postgres>,
    mut operation: F,
) -> Result<T, auction_application::core::RepositoryError>
where
    T: Send,
    F: for<'a> FnMut(
            &'a mut sqlx::Transaction<'static, Postgres>,
        ) -> RecoveryActionTransactionFuture<'a, T>
        + Send,
{
    super::run_serializable_action_transaction(pool, |transaction| operation(transaction.native()))
        .await
}
