//! Public realtime consumer key 的进程生命周期互斥。

#[cfg(feature = "sqlite")]
use super::super::sqlite_ownership::SqliteConsumerLock;
use thiserror::Error;

use super::super::DatabaseStore;

/// 将 consumer key 的互斥与全部 relay 数据访问绑定到同一生命周期。
///
/// PostgreSQL 消费事务只使用持锁连接，连接失效后不会从池重新领取连接继续确认游标；
/// SQLite 在所有消费操作完成前持续持有同主机文件锁。
pub struct RealtimeConsumerOwnership {
    pub(super) consumer_key: String,
    pub(super) backend: OwnedRealtimeBackend,
}

pub(super) enum OwnedRealtimeBackend {
    #[cfg(feature = "postgres")]
    Postgres(futures_util::lock::Mutex<sqlx::pool::PoolConnection<sqlx::Postgres>>),
    #[cfg(feature = "sqlite")]
    Sqlite { store: super::super::SqliteStore, _lock: SqliteConsumerLock },
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
    /// PostgreSQL 用专用 session advisory lock，SQLite 用同主机非阻塞文件锁强制唯一 owner。
    pub async fn acquire_realtime_consumer_ownership(
        &self,
        consumer_key: &str,
    ) -> Result<RealtimeConsumerOwnership, RealtimeConsumerOwnershipError> {
        if consumer_key.is_empty()
            || consumer_key.len() > 128
            || consumer_key != consumer_key.trim()
            || consumer_key.chars().any(char::is_control)
        {
            return Err(RealtimeConsumerOwnershipError::InvalidKey);
        }
        match self {
            #[cfg(feature = "postgres")]
            DatabaseStore::Postgres(store) => {
                let mut connection = store
                    .raw_pool()
                    .acquire()
                    .await
                    .map_err(|_| RealtimeConsumerOwnershipError::Unavailable)?;
                // 在锁查询前设置：响应到达前取消也必须关闭 session，不能把已持锁连接归池。
                connection.close_on_drop();
                let lock_name = format!("auction/public-realtime/v1/{consumer_key}");
                let acquired = sqlx::query_scalar::<_, bool>(
                    "SELECT pg_try_advisory_lock(hashtextextended($1, 0))",
                )
                .bind(lock_name)
                .fetch_one(&mut *connection)
                .await
                .map_err(|_| RealtimeConsumerOwnershipError::Unavailable)?;
                if !acquired {
                    return Err(RealtimeConsumerOwnershipError::AlreadyOwned);
                }
                Ok(RealtimeConsumerOwnership {
                    consumer_key: consumer_key.to_owned(),
                    backend: OwnedRealtimeBackend::Postgres(futures_util::lock::Mutex::new(
                        connection,
                    )),
                })
            }
            #[cfg(feature = "sqlite")]
            DatabaseStore::Sqlite(store) => acquire_sqlite_ownership(store, consumer_key),
        }
    }
}

#[cfg(feature = "sqlite")]
fn acquire_sqlite_ownership(
    store: &super::super::SqliteStore,
    consumer_key: &str,
) -> Result<RealtimeConsumerOwnership, RealtimeConsumerOwnershipError> {
    let lock = store.ownership().acquire(consumer_key)?;
    Ok(RealtimeConsumerOwnership {
        consumer_key: consumer_key.to_owned(),
        backend: OwnedRealtimeBackend::Sqlite { store: store.clone(), _lock: lock },
    })
}
