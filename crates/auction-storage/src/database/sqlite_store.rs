//! SQLite Lite 连接池与文件/内存数据库边界。

use super::sqlite_ownership::SqliteOwnershipIdentity;
use super::*;
use sqlx::ConnectOptions;

/// SQLite Lite 适配器边界。
///
/// 文件数据库使用 WAL：读事务不阻塞单个写事务，符合 Lite 档「低并发但仍要可用」的定位。
/// 写入串行由 SQLite 自身的写锁保证，`busy_timeout` 决定等锁而不是立刻返回 `SQLITE_BUSY`。
///
/// 内存数据库不落盘，WAL 无意义且必须保持单连接——每个新连接都会得到独立的空库。
#[cfg(feature = "sqlite")]
#[derive(Debug, Clone)]
pub struct SqliteStore {
    pool: TrackedPool<Sqlite>,
    ownership: Arc<SqliteOwnershipIdentity>,
}

/// 等待 SQLite 写锁的上限。超过该时长仍拿不到锁才返回错误，避免瞬时争用直接失败。
#[cfg(feature = "sqlite")]
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// 文件数据库的连接上限。WAL 允许多个读连接与一个写连接并存。
#[cfg(feature = "sqlite")]
const SQLITE_FILE_MAX_CONNECTIONS: u32 = 5;

#[cfg(feature = "sqlite")]
impl SqliteStore {
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let mut options: SqliteConnectOptions = database_url.parse()?;
        let in_memory = options
            .to_url_lossy()
            .query_pairs()
            .any(|(key, value)| key == "mode" && value == "memory");

        options = options
            // Lite 档面向本地与单主机部署，数据库文件不存在时创建而不是启动失败。
            .create_if_missing(true)
            .busy_timeout(SQLITE_BUSY_TIMEOUT)
            // 外键约束默认关闭，必须显式开启，否则 Lite 档会静默接受违反引用完整性的写入。
            .foreign_keys(true);

        if !in_memory {
            options = options.journal_mode(SqliteJournalMode::Wal);
        }

        // 内存库必须单连接：新连接会得到各自独立的空库，池化会让数据看起来随机消失。
        let max_connections = if in_memory { 1 } else { SQLITE_FILE_MAX_CONNECTIONS };
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .connect_with(options.clone())
            .await?;
        let ownership = Arc::new(SqliteOwnershipIdentity::from_connected(&pool, &options).await?);
        Ok(Self { pool: TrackedPool::new(pool), ownership })
    }

    pub(crate) fn pool(&self) -> &TrackedPool<Sqlite> {
        &self.pool
    }

    pub(crate) fn raw_pool(&self) -> &SqlitePool {
        self.pool.raw()
    }

    pub(super) fn ownership(&self) -> &SqliteOwnershipIdentity {
        &self.ownership
    }
}
