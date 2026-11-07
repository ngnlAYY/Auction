//! PostgreSQL 提交后唤醒提示；只消费固定空载荷频道，持久任务真值仍由 claim 查询决定。
//!
//! 发送端不在业务事务内：插入 `lot_watch` 投递的事务提交后，由完成插入的进程单独执行
//! [`publish_after_commit`]；失败只降低及时性，Worker 轮询仍会领取已提交投递。

use auction_application::auction::ApplicationFuture;
use auction_application::delivery::DeliveryWakeup;
use sqlx::postgres::PgListener;

struct PostgresDeliveryWakeup {
    listener: PgListener,
    _guard: Option<auction_observability::DatabaseConnectionGuard>,
}

/// 业务事务提交后单独发送唤醒；独立自动提交语句不延长原事务的提交。
pub(crate) async fn publish_after_commit(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
) -> Result<(), auction_application::core::ApplicationError> {
    sqlx::query("SELECT public.signal_delivery_ready()")
        .execute(pool.raw())
        .await
        .map(|_| ())
        .map_err(|error| super::map_storage_error(error).into())
}

pub(super) async fn subscribe(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
) -> Result<Box<dyn DeliveryWakeup>, auction_application::core::ApplicationError> {
    let mut listener =
        PgListener::connect_with(pool.raw()).await.map_err(super::map_storage_error)?;
    listener.listen("auction_delivery_ready").await.map_err(super::map_storage_error)?;
    Ok(Box::new(PostgresDeliveryWakeup {
        listener,
        _guard: pool
            .recorder()
            .reserve(auction_observability::DatabaseReservedPurpose::DeliveryListener),
    }))
}

impl DeliveryWakeup for PostgresDeliveryWakeup {
    fn wait(&mut self) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            // 断线向上层报告；重建订阅后主动扫描一次，填补重连期间的提示缺口。
            self.listener.try_recv().await.map_err(super::map_storage_error)?;
            Ok(())
        })
    }
}
