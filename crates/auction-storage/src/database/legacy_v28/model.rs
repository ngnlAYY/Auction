//! Auction-BF v28 接管的公开安全投影与错误分类；不含备份文件或外部运维 receipt。

use chrono::{DateTime, Utc};
use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

/// 旧版 v28 接管的只读证据。
#[derive(Debug, Clone, Serialize)]
pub struct PostgresLegacyV28Preflight {
    pub format: &'static str,
    pub collected_at: DateTime<Utc>,
    pub database_name: String,
    pub server_version: String,
    pub server_version_num: i32,
    pub in_recovery: bool,
    pub default_transaction_read_only: bool,
    pub active_other_sessions: i64,
    pub sqlx_history_present: bool,
    pub legacy_history_present: bool,
    pub legacy_history_row_count: i64,
    pub legacy_version: Option<i64>,
    pub legacy_dirty: Option<bool>,
    pub schema_identity_valid: bool,
    pub expected_table_count: usize,
    pub actual_table_count: usize,
    pub missing_tables: Vec<String>,
    pub unexpected_tables: Vec<String>,
    pub source_shape_issue_count: i64,
    pub bridge_target_object_count: i64,
    pub policy_issue_count: i64,
    pub lot_projection_issue_count: i64,
    pub bid_event_issue_count: i64,
    pub adoption_ready: bool,
    pub notes: Vec<String>,
}

/// 一次成功接管的数据库侧结果；文件备份与不可覆盖 receipt 由 `auctionctl` 拥有。
#[derive(Debug, Clone, Serialize)]
pub struct PostgresLegacyV28Adoption {
    pub adopted_at: DateTime<Utc>,
    pub database_name: String,
    pub legacy_version: i64,
    pub baselined_sqlx_versions: Vec<i64>,
    pub final_sqlx_version: i64,
    pub audit_id: Uuid,
}

/// 旧版 v28 接管的安全错误分类。
#[derive(Debug, Error)]
pub enum PostgresLegacyV28Error {
    #[error("confirmed PostgreSQL database name is invalid")]
    InvalidDatabaseName,

    #[error("confirmed PostgreSQL database does not match the connected database")]
    DatabaseMismatch { expected: String, actual: String },

    #[error("legacy v28 adoption preflight did not pass")]
    PreflightRejected,

    #[error("failed to query legacy v28 adoption state at {stage}")]
    Query {
        stage: &'static str,
        #[source]
        source: sqlx::Error,
    },

    #[error("failed to acquire or release the SQLx migration advisory lock")]
    MigrationLock {
        #[source]
        source: sqlx::migrate::MigrateError,
    },

    #[error("failed to apply the transactional legacy v28 bridge")]
    Bridge {
        #[source]
        source: sqlx::Error,
    },

    #[error(
        "legacy v28 bridge committed, but a later forward migration failed; rerun auctionctl migrate"
    )]
    ForwardMigration {
        #[source]
        source: sqlx::migrate::MigrateError,
    },

    #[error("embedded PostgreSQL migration baseline is incomplete")]
    EmbeddedBaselineIncomplete,
}
