//! Public realtime consumer key 的进程生命周期互斥。

#[cfg(feature = "sqlite")]
use std::fs::{File, OpenOptions};
#[cfg(all(feature = "sqlite", unix))]
use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};

#[cfg(feature = "sqlite")]
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::super::DatabaseStore;

/// 持有 consumer key 的数据库 session；guard 存活期间同库中的重复实例必须启动失败。
pub enum RealtimeConsumerOwnership {
    #[cfg(feature = "postgres")]
    Postgres(sqlx::pool::PoolConnection<sqlx::Postgres>),
    #[cfg(feature = "sqlite")]
    Sqlite { _lock_file: File },
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
                Ok(RealtimeConsumerOwnership::Postgres(connection))
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
    #[cfg(unix)]
    {
        let lock_identity = format!("{}:{consumer_key}", store.ownership_namespace());
        let digest = Sha256::digest(lock_identity.as_bytes());
        let lock_path =
            std::env::temp_dir().join(format!("auction-realtime-consumer-{:x}.lock", digest));
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(lock_path)
            .map_err(|_| RealtimeConsumerOwnershipError::Unavailable)?;
        // SAFETY: fd 在本 guard 内保持有效；flock 不取得 Rust 内存别名，也不读写文件内容。
        let result = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::WouldBlock {
                Err(RealtimeConsumerOwnershipError::AlreadyOwned)
            } else {
                Err(RealtimeConsumerOwnershipError::Unavailable)
            };
        }
        Ok(RealtimeConsumerOwnership::Sqlite { _lock_file: lock_file })
    }
    #[cfg(not(unix))]
    {
        let _ = (store, consumer_key);
        Err(RealtimeConsumerOwnershipError::Unavailable)
    }
}
