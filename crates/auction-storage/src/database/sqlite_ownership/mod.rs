//! SQLite 实际数据库身份与 relay 消费锁；文件锁不能依赖 URL 写法或私有临时目录。

use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::{Arc, Mutex, OnceLock, Weak},
};

use sqlx::{ConnectOptions, SqlitePool, sqlite::SqliteConnectOptions};

use super::realtime_consumer::RealtimeConsumerOwnershipError;

#[cfg(unix)]
mod file;

type MemoryClaims = Mutex<HashSet<String>>;
type MemoryRegistry = Mutex<HashMap<String, Weak<MemoryClaims>>>;
static MEMORY_REGISTRY: OnceLock<MemoryRegistry> = OnceLock::new();

/// Store 克隆共享同一内存身份；文件身份从连接实际打开的 main 数据库取得。
pub(super) struct SqliteOwnershipIdentity {
    backend: Backend,
}

enum Backend {
    #[cfg(unix)]
    File(file::DatabaseFile),
    Memory(Arc<MemoryClaims>),
    Unavailable,
}

impl fmt::Debug for SqliteOwnershipIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("SqliteOwnershipIdentity").finish_non_exhaustive()
    }
}

impl SqliteOwnershipIdentity {
    pub(super) async fn from_connected(
        pool: &SqlitePool,
        options: &SqliteConnectOptions,
    ) -> Result<Self, sqlx::Error> {
        let effective = options.to_url_lossy();
        if effective.query_pairs().any(|(key, _)| key == "vfs") {
            return Ok(Self { backend: Backend::Unavailable });
        }
        // 只通过既有 SQLite 连接读取路径。额外打开再关闭数据库文件可能释放 SQLite 的进程锁。
        let filename: String =
            sqlx::query_scalar("SELECT file FROM pragma_database_list WHERE name = 'main'")
                .fetch_one(pool)
                .await?;
        let backend = if filename.is_empty() {
            let memory =
                effective.query_pairs().any(|(key, value)| key == "mode" && value == "memory");
            if !memory {
                Backend::Unavailable
            } else {
                let shared =
                    effective.query_pairs().any(|(key, value)| key == "cache" && value == "shared");
                let immutable = effective.query_pairs().any(|(key, _)| key == "immutable");
                memory_backend(options, shared, immutable)?
            }
        } else {
            #[cfg(unix)]
            {
                Backend::File(file::DatabaseFile::new(&filename).map_err(|_| {
                    sqlx::Error::Protocol("无法确认 SQLite 数据库文件身份".to_owned())
                })?)
            }
            #[cfg(not(unix))]
            {
                Backend::Unavailable
            }
        };
        Ok(Self { backend })
    }

    pub(super) fn acquire(
        &self,
        consumer_key: &str,
    ) -> Result<SqliteConsumerLock, RealtimeConsumerOwnershipError> {
        match &self.backend {
            #[cfg(unix)]
            Backend::File(database) => database.acquire(consumer_key),
            Backend::Memory(claims) => {
                let inserted = claims
                    .lock()
                    .map_err(|_| RealtimeConsumerOwnershipError::Unavailable)?
                    .insert(consumer_key.to_owned());
                if !inserted {
                    return Err(RealtimeConsumerOwnershipError::AlreadyOwned);
                }
                Ok(SqliteConsumerLock::Memory {
                    claims: Arc::clone(claims),
                    key: consumer_key.to_owned(),
                })
            }
            Backend::Unavailable => Err(RealtimeConsumerOwnershipError::Unavailable),
        }
    }
}

fn memory_backend(
    options: &SqliteConnectOptions,
    shared: bool,
    immutable: bool,
) -> Result<Backend, sqlx::Error> {
    let name = options
        .get_filename()
        .to_str()
        .ok_or_else(|| sqlx::Error::Protocol("无法确认 SQLite 内存数据库身份".to_owned()))?;
    // immutable 会让 SQLx 再包装一层 file: URI，不能沿用未包装名称猜测共享缓存身份。
    if immutable {
        return Ok(Backend::Unavailable);
    }
    let Some(name) = name.strip_prefix("file:") else {
        // SQLite 对非 URI 的内存名称不共享缓存，即使 SQLx 的 shared_cache 标志为 true。
        return Ok(Backend::Memory(Arc::new(Mutex::new(HashSet::new()))));
    };
    // SQLx 为 :memory: 分配唯一 file:sqlx-in-memory-N 名称；显式命名共享库沿用有效名称。
    // 只接纳不含 URI 转义或路径语义的名称，不能猜测特殊 URI/VFS 是否指向同一共享缓存。
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Ok(Backend::Unavailable);
    }
    // 特殊 URI 必须先拒绝；内层 cache=shared 否则可能覆盖外层 cache=private。
    if !shared {
        return Ok(Backend::Memory(Arc::new(Mutex::new(HashSet::new()))));
    }
    let mut registry = MEMORY_REGISTRY
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| sqlx::Error::Protocol("SQLite 内存消费锁不可用".to_owned()))?;
    registry.retain(|_, claims| claims.strong_count() > 0);
    let claims = registry.get(name).and_then(Weak::upgrade).unwrap_or_else(|| {
        let claims = Arc::new(Mutex::new(HashSet::new()));
        registry.insert(name.to_owned(), Arc::downgrade(&claims));
        claims
    });
    Ok(Backend::Memory(claims))
}

/// 文件 sidecar 不删除；内存锁只释放当前 consumer key，不影响其他 Store 或消费者。
pub(super) enum SqliteConsumerLock {
    #[cfg(unix)]
    File {
        _file: std::fs::File,
    },
    Memory {
        claims: Arc<MemoryClaims>,
        key: String,
    },
}

impl Drop for SqliteConsumerLock {
    fn drop(&mut self) {
        match self {
            Self::Memory { claims, key } => {
                if let Ok(mut claims) = claims.lock() {
                    claims.remove(key);
                }
            }
            #[cfg(unix)]
            Self::File { .. } => {}
        }
    }
}
