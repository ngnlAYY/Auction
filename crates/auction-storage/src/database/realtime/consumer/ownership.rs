//! Public realtime consumer key 的进程生命周期互斥。

use thiserror::Error;

use crate::database::DatabaseStore;

/// 将 consumer key 的互斥与全部 relay 数据访问绑定到同一生命周期：消费事务只使用 PostgreSQL
/// 持锁连接，连接失效后不得重新取连接继续确认游标。
pub struct RealtimeConsumerOwnership {
    pub(super) consumer_key: String,
    pub(super) backend: OwnedRealtimeBackend,
}

pub(super) enum OwnedRealtimeBackend {
    Postgres(futures_util::lock::Mutex<crate::database::TrackedConnection<sqlx::Postgres>>),
}

/// realtime consumer key 互斥获取失败。
#[derive(Debug, Error)]
pub enum RealtimeConsumerOwnershipError {
    #[error("realtime consumer key 无效")]
    InvalidKey,
    #[error("realtime consumer key 已被另一个活动实例占用")]
    AlreadyOwned,
    #[error("realtime consumer key 互斥状态不可用")]
    Unavailable,
}

impl DatabaseStore {
    /// 以 PostgreSQL 专用 session advisory lock 强制唯一 owner。
    pub async fn acquire_realtime_consumer_ownership(
        &self,
        consumer_key: &str,
    ) -> Result<RealtimeConsumerOwnership, RealtimeConsumerOwnershipError> {
        if !auction_domain::valid_text(consumer_key, 128) {
            return Err(RealtimeConsumerOwnershipError::InvalidKey);
        }
        let mut connection = self
            .pool()
            .acquire_reserved(auction_observability::DatabaseReservedPurpose::RealtimeOwnership)
            .await
            .map_err(|_| RealtimeConsumerOwnershipError::Unavailable)?;
        // 在锁查询前设置：响应到达前取消也必须关闭 session，不能把已持锁连接归池。
        connection.close_on_drop();
        let lock_name = format!("auction/public-realtime/v1/{consumer_key}");
        let acquired =
            sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
                .bind(lock_name)
                .fetch_one(&mut *connection)
                .await
                .map_err(|_| RealtimeConsumerOwnershipError::Unavailable)?;
        if !acquired {
            return Err(RealtimeConsumerOwnershipError::AlreadyOwned);
        }
        Ok(RealtimeConsumerOwnership {
            consumer_key: consumer_key.to_owned(),
            backend: OwnedRealtimeBackend::Postgres(futures_util::lock::Mutex::new(connection)),
        })
    }
}
