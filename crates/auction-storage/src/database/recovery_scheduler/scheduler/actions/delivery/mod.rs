//! Recovery action 投递状态推进与结果观察的模块装配；pending → prepared → enqueued 的身份推进和
//! 终态证明由子模块负责，本模块保留稳定出口与原生事务借用适配。

#[cfg(feature = "postgres")]
use {
    crate::database::recovery_scheduler::actions::RecoveryActionTransactionFuture, sqlx::Postgres,
};

#[cfg(feature = "postgres")]
mod activation;
#[cfg(feature = "postgres")]
mod observation;
#[cfg(feature = "postgres")]
mod prepare;

#[cfg(feature = "postgres")]
pub(crate) use activation::mark_action_enqueued_postgres;
#[cfg(feature = "postgres")]
pub(crate) use observation::observe_action_postgres;
#[cfg(feature = "postgres")]
pub(crate) use prepare::link_action_postgres;

/// 原生观察回调仍借用外层受跟踪的同一事务，不另取连接或复制重试循环。
#[cfg(feature = "postgres")]
async fn run_serializable_action_transaction<T, F>(
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
    super::run_serializable_action_transaction(pool, |transaction| operation(transaction.native()))
        .await
}
