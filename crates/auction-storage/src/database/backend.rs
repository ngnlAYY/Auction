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
        // 后端选择也拒绝缩略 URI 和坏编码；具体目标由连接边界继续校验。
        if database_url != database_url.trim() || !valid_url_encoding(database_url) {
            return Err(StorageError::InvalidPostgresUrl);
        }
        let url = url::Url::parse(database_url).map_err(|_| StorageError::InvalidPostgresUrl)?;
        match url.scheme() {
            "postgres" | "postgresql" => {
                let hierarchical = database_url
                    .split_once(':')
                    .is_some_and(|(_, remainder)| remainder.starts_with("//"));
                if !hierarchical || url.cannot_be_a_base() || url.fragment().is_some() {
                    return Err(StorageError::InvalidPostgresUrl);
                }
                Ok(Self::Postgres)
            }
            scheme => Err(StorageError::UnsupportedScheme(scheme.to_owned())),
        }
    }
}

/// 不向驱动传入会被容忍或有损解码的 URI，错误只报告固定分类。
fn valid_url_encoding(value: &str) -> bool {
    let mut bytes = value.bytes();
    let mut decoded = Vec::with_capacity(value.len());
    while let Some(byte) = bytes.next() {
        let byte = if byte == b'%' {
            let Some(high) = bytes.next().and_then(|byte| char::from(byte).to_digit(16)) else {
                return false;
            };
            let Some(low) = bytes.next().and_then(|byte| char::from(byte).to_digit(16)) else {
                return false;
            };
            (high * 16 + low) as u8
        } else {
            byte
        };
        decoded.push(byte);
    }
    std::str::from_utf8(&decoded).is_ok_and(|value| !value.chars().any(char::is_control))
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

    #[error("PostgreSQL URL 必须是无 fragment、编码有效的 postgres:// 或 postgresql:// URI")]
    InvalidPostgresUrl,

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
