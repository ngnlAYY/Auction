//! PostgreSQL 提交后唤醒提示；只消费固定空载荷频道，持久任务真值仍由 claim 查询决定。

use auction_application::{ApplicationFuture, DeliveryWakeup};
use sqlx::postgres::PgListener;

struct PostgresDeliveryWakeup(PgListener);

pub(super) async fn subscribe(
    pool: &sqlx::PgPool,
) -> Result<Box<dyn DeliveryWakeup>, auction_application::ApplicationError> {
    let mut listener = PgListener::connect_with(pool).await.map_err(super::map_storage_error)?;
    listener.listen("auction_delivery_ready").await.map_err(super::map_storage_error)?;
    Ok(Box::new(PostgresDeliveryWakeup(listener)))
}

impl DeliveryWakeup for PostgresDeliveryWakeup {
    fn wait(&mut self) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            // 断线向上层报告；重建订阅后主动扫描一次，填补重连期间的提示缺口。
            self.0.try_recv().await.map_err(super::map_storage_error)?;
            Ok(())
        })
    }
}
