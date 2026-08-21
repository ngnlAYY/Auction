use std::fmt;

use thiserror::Error;

#[cfg(feature = "postgres")]
use sqlx::{PgPool, postgres::PgPoolOptions};
#[cfg(feature = "sqlite")]
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};

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

/// SQLite Lite 适配器边界。单连接保证 `sqlite::memory:` 行为确定，并在仓储事务策略完成前
/// 串行化写入。
#[cfg(feature = "sqlite")]
#[derive(Debug)]
pub struct SqliteStore {
    pool: SqlitePool,
}

#[cfg(feature = "sqlite")]
impl SqliteStore {
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = SqlitePoolOptions::new().max_connections(1).connect(database_url).await?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
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
