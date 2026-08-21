use std::fmt;
#[cfg(feature = "sqlite")]
use std::time::Duration;

use thiserror::Error;

#[cfg(feature = "postgres")]
use sqlx::{PgPool, postgres::PgPoolOptions};
#[cfg(feature = "sqlite")]
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseBackend {
    Postgres,
    Sqlite,
}

impl DatabaseBackend {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Sqlite => "sqlite",
        }
    }

    pub fn from_url(database_url: &str) -> Result<Self, StorageError> {
        let scheme = database_url.split(':').next().unwrap_or_default().to_ascii_lowercase();

        match scheme.as_str() {
            "postgres" | "postgresql" => Ok(Self::Postgres),
            "sqlite" => Ok(Self::Sqlite),
            _ => Err(StorageError::UnsupportedScheme(scheme)),
        }
    }
}

impl fmt::Display for DatabaseBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error(
        "unsupported database URL scheme {0:?}; expected postgres://, postgresql://, or sqlite:"
    )]
    UnsupportedScheme(String),

    #[error("failed to connect to {backend} database")]
    Connection {
        backend: DatabaseBackend,
        #[source]
        source: sqlx::Error,
    },

    #[error("{backend} storage support is not compiled; enable its Cargo feature")]
    BackendNotCompiled { backend: DatabaseBackend },
}

/// 运行时选择的数据库边界。领域层和应用层不得依赖该枚举，只能使用仓储与事务端口。
#[cfg(any(feature = "postgres", feature = "sqlite"))]
#[derive(Debug)]
pub enum DatabaseStore {
    #[cfg(feature = "postgres")]
    Postgres(PostgresStore),
    #[cfg(feature = "sqlite")]
    Sqlite(SqliteStore),
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
impl DatabaseStore {
    pub async fn connect(database_url: &str) -> Result<Self, StorageError> {
        let backend = DatabaseBackend::from_url(database_url)?;

        match backend {
            DatabaseBackend::Postgres => {
                #[cfg(feature = "postgres")]
                {
                    PostgresStore::connect(database_url)
                        .await
                        .map(Self::Postgres)
                        .map_err(|source| StorageError::Connection { backend, source })
                }

                #[cfg(not(feature = "postgres"))]
                {
                    Err(StorageError::BackendNotCompiled { backend })
                }
            }
            DatabaseBackend::Sqlite => {
                #[cfg(feature = "sqlite")]
                {
                    SqliteStore::connect(database_url)
                        .await
                        .map(Self::Sqlite)
                        .map_err(|source| StorageError::Connection { backend, source })
                }

                #[cfg(not(feature = "sqlite"))]
                {
                    Err(StorageError::BackendNotCompiled { backend })
                }
            }
        }
    }

    pub const fn backend(&self) -> DatabaseBackend {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => DatabaseBackend::Postgres,
            #[cfg(feature = "sqlite")]
            Self::Sqlite(_) => DatabaseBackend::Sqlite,
        }
    }

    #[cfg(feature = "postgres")]
    pub fn postgres_pool(&self) -> Option<&PgPool> {
        match self {
            Self::Postgres(store) => Some(store.pool()),
            #[cfg(feature = "sqlite")]
            Self::Sqlite(_) => None,
        }
    }

    #[cfg(feature = "sqlite")]
    pub fn sqlite_pool(&self) -> Option<&SqlitePool> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => None,
            Self::Sqlite(store) => Some(store.pool()),
        }
    }
}

/// PostgreSQL 适配器边界。只有在新 schema 与事务不变量完成评审后才新增仓储实现。
#[cfg(feature = "postgres")]
#[derive(Debug)]
pub struct PostgresStore {
    pool: PgPool,
}

#[cfg(feature = "postgres")]
impl PostgresStore {
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new().max_connections(16).connect(database_url).await?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

/// SQLite Lite 适配器边界。
///
/// 文件数据库使用 WAL：读事务不阻塞单个写事务，符合 Lite 档「低并发但仍要可用」的定位。
/// 写入串行由 SQLite 自身的写锁保证，`busy_timeout` 决定等锁而不是立刻返回 `SQLITE_BUSY`。
///
/// 内存数据库不落盘，WAL 无意义且必须保持单连接——每个新连接都会得到独立的空库。
#[cfg(feature = "sqlite")]
#[derive(Debug)]
pub struct SqliteStore {
    pool: SqlitePool,
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
        let in_memory = is_in_memory_sqlite(database_url);
        let mut options: SqliteConnectOptions = database_url.parse()?;

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
        let pool =
            SqlitePoolOptions::new().max_connections(max_connections).connect_with(options).await?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

/// 判断 SQLite URL 是否指向内存数据库。
///
/// SQLx 的 `in_memory` 字段是 `pub(crate)`，外部无法读取，因此这里按它的解析规则识别两种
/// 写法：`:memory:` 路径和 `mode=memory` 查询参数。判断结果只用于选择 journal 模式与连接数，
/// 误判为文件库的后果是给内存库设了无效的 WAL，不影响正确性。
#[cfg(feature = "sqlite")]
fn is_in_memory_sqlite(database_url: &str) -> bool {
    let without_scheme = database_url
        .strip_prefix("sqlite://")
        .or_else(|| database_url.strip_prefix("sqlite:"))
        .unwrap_or(database_url);
    let (path, query) = match without_scheme.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (without_scheme, None),
    };

    if path == ":memory:" {
        return true;
    }

    query.is_some_and(|query| {
        query.split('&').any(|pair| pair.split_once('=') == Some(("mode", "memory")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_supported_database_urls() {
        assert_eq!(DatabaseBackend::from_url("postgres://db").unwrap(), DatabaseBackend::Postgres);
        assert_eq!(
            DatabaseBackend::from_url("postgresql://db").unwrap(),
            DatabaseBackend::Postgres
        );
        assert_eq!(DatabaseBackend::from_url("sqlite::memory:").unwrap(), DatabaseBackend::Sqlite);
    }

    #[test]
    fn rejects_unknown_database_urls_without_echoing_credentials() {
        let error = DatabaseBackend::from_url("mysql://user:secret@example/db").unwrap_err();

        assert!(matches!(error, StorageError::UnsupportedScheme(ref scheme) if scheme == "mysql"));
        assert!(!error.to_string().contains("secret"));
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn connects_to_sqlite_memory_database() {
        let store = DatabaseStore::connect("sqlite::memory:").await.unwrap();

        assert_eq!(store.backend(), DatabaseBackend::Sqlite);
        assert!(store.sqlite_pool().is_some());
        #[cfg(feature = "postgres")]
        assert!(store.postgres_pool().is_none());
    }
}
