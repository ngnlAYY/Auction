//! PostgreSQL 提交后唤醒提示；只消费固定空载荷频道，持久任务真值仍由 claim 查询决定。
//!
//! 发送端不在业务事务内：插入 `lot_watch` 投递的事务、以及含已入队回信的入站邮件批次提交后，
//! 由完成提交的进程单独执行 [`publish_after_commit`]；失败只降低及时性，Worker 轮询仍会领取
//! 已提交投递。

use auction_application::core::ApplicationFuture;
use auction_application::delivery::DeliveryWakeup;
use sqlx::postgres::PgListener;

use super::mapping::map_storage_error;

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
        .map_err(|error| map_storage_error(error).into())
}

pub(super) async fn subscribe(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
) -> Result<Box<dyn DeliveryWakeup>, auction_application::core::ApplicationError> {
    let mut listener = PgListener::connect_with(pool.raw()).await.map_err(map_storage_error)?;
    listener.listen("auction_delivery_ready").await.map_err(map_storage_error)?;
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
            // `try_recv` 只在建立连接后等待下一条协议消息，`Ok(None)` 表示连接已断（或已重连），
            // 不能伪装成一次提示，否则上层会当作正常唤醒并失去重连/补扫信号。
            match self.listener.try_recv().await.map_err(map_storage_error)? {
                Some(_) => Ok(()),
                None => Err(crate::database::storage_error("投递唤醒订阅连接已断开")),
            }
        })
    }
}
