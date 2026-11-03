//! PostgreSQL 提交后唤醒提示；只消费固定空载荷频道，持久任务真值仍由 claim 查询决定。

use auction_application::{ApplicationFuture, DeliveryWakeup};
use sqlx::postgres::PgListener;

struct PostgresDeliveryWakeup {
    listener: PgListener,
    _guard: Option<auction_observability::DatabaseConnectionGuard>,
}

pub(super) async fn subscribe(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
) -> Result<Box<dyn DeliveryWakeup>, auction_application::ApplicationError> {
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
