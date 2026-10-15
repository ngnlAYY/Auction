//! 数据库后端枚举与统一存储错误。

use std::fmt;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseBackend {
    Postgres,
}

impl DatabaseBackend {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
        }
    }

    pub fn from_url(database_url: &str) -> Result<Self, StorageError> {
        let scheme = database_url.split(':').next().unwrap_or_default().to_ascii_lowercase();

        match scheme.as_str() {
            "postgres" | "postgresql" => Ok(Self::Postgres),
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
    #[error("unsupported database URL scheme {0:?}; expected postgres:// or postgresql://")]
    UnsupportedScheme(String),

    #[error("failed to connect to {backend} database")]
    Connection {
        backend: DatabaseBackend,
        #[source]
        source: sqlx::Error,
    },

    #[error("failed to migrate {backend} database")]
    Migration {
        backend: DatabaseBackend,
        #[source]
        source: sqlx::migrate::MigrateError,
    },

    #[error("{backend} migration source invariant failed: {detail}")]
    MigrationSourceInvariant { backend: DatabaseBackend, detail: &'static str },

    #[error("failed to inspect {backend} migration target")]
    MigrationTarget {
        backend: DatabaseBackend,
        #[source]
        source: sqlx::Error,
    },

    #[error("refusing to migrate an unknown non-empty {backend} database")]
    UnknownDatabase { backend: DatabaseBackend },
}
