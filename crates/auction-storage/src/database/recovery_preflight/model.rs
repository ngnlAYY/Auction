//! PostgreSQL 恢复前预检的结果模型与错误分类。

use chrono::{DateTime, Utc};
use serde::Serialize;
use thiserror::Error;

/// 恢复前预检证据的机器可读格式标识。
pub const RECOVERY_PREFLIGHT_FORMAT: &str = "auction-postgres-recovery-preflight.v1";

/// PostgreSQL 恢复前预检结果。
#[derive(Debug, Serialize)]
pub struct PostgresRecoveryPreflight {
    pub format: &'static str,
    pub collected_at: DateTime<Utc>,
    pub database_name: String,
    pub server_version: String,
    pub in_recovery: bool,
    pub default_transaction_read_only: bool,
    pub restore_write_possible: bool,
    pub active_other_sessions: i64,
    pub database_size_bytes: i64,
    pub user_object_count: i64,
    pub schema_identity_present: bool,
    pub schema_identity: Option<super::PostgresSchemaIdentity>,
    pub migration_history_present: bool,
    pub migration: Option<super::PostgresMigrationInventory>,
    pub restore_preflight_passed: bool,
    pub notes: Vec<String>,
}

/// 恢复前预检失败的安全错误分类。
#[derive(Debug, Error)]
pub enum RecoveryPreflightError {
    #[error("confirmed PostgreSQL database name is invalid")]
    InvalidDatabaseName,

    #[error("confirmed PostgreSQL database does not match the connected database")]
    DatabaseMismatch { expected: String, actual: String },

    #[error("Auction schema identity is present but invalid")]
    SchemaIdentityInvalid,

    #[error("failed to query PostgreSQL recovery preflight at {stage}")]
    Query {
        stage: &'static str,
        #[source]
        source: sqlx::Error,
    },

    #[error("failed to finish PostgreSQL recovery preflight transaction")]
    Commit {
        #[source]
        source: sqlx::Error,
    },
}
