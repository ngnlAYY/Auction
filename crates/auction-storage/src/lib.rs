use std::fmt;

use sqlx::{PgPool, SqlitePool, postgres::PgPoolOptions, sqlite::SqlitePoolOptions};
use thiserror::Error;

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
}

/// Runtime-selected database boundary. Domain and application code must not
/// depend on this enum; they use repository and transaction ports instead.
#[derive(Debug)]
pub enum DatabaseStore {
    Postgres(PostgresStore),
    Sqlite(SqliteStore),
}

impl DatabaseStore {
    pub async fn connect(database_url: &str) -> Result<Self, StorageError> {
        let backend = DatabaseBackend::from_url(database_url)?;

        match backend {
            DatabaseBackend::Postgres => PostgresStore::connect(database_url)
                .await
                .map(Self::Postgres)
                .map_err(|source| StorageError::Connection { backend, source }),
            DatabaseBackend::Sqlite => SqliteStore::connect(database_url)
                .await
                .map(Self::Sqlite)
                .map_err(|source| StorageError::Connection { backend, source }),
        }
    }

    pub const fn backend(&self) -> DatabaseBackend {
        match self {
            Self::Postgres(_) => DatabaseBackend::Postgres,
            Self::Sqlite(_) => DatabaseBackend::Sqlite,
        }
    }

    pub fn postgres_pool(&self) -> Option<&PgPool> {
        match self {
            Self::Postgres(store) => Some(store.pool()),
            Self::Sqlite(_) => None,
        }
    }

    pub fn sqlite_pool(&self) -> Option<&SqlitePool> {
        match self {
            Self::Postgres(_) => None,
            Self::Sqlite(store) => Some(store.pool()),
        }
    }
}

/// PostgreSQL adapter boundary. Repository implementations will be added only
/// after the new schema and transaction invariants are reviewed.
#[derive(Debug)]
pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new().max_connections(16).connect(database_url).await?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

/// SQLite adapter boundary. A single pooled connection keeps `sqlite::memory:`
/// deterministic and serializes writes until the repository transaction policy
/// is implemented.
#[derive(Debug)]
pub struct SqliteStore {
    pool: SqlitePool,
}

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

    #[tokio::test]
    async fn connects_to_sqlite_memory_database() {
        let store = DatabaseStore::connect("sqlite::memory:").await.unwrap();

        assert_eq!(store.backend(), DatabaseBackend::Sqlite);
        assert!(store.sqlite_pool().is_some());
        assert!(store.postgres_pool().is_none());
    }
}
