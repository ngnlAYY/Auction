//! 数据库重建的预检、结果与错误模型。

use chrono::{DateTime, Utc};
use serde::Serialize;
use thiserror::Error;

/// 数据库重建预检的机器可读格式标识。
pub const POSTGRES_REBUILD_PREFLIGHT_FORMAT: &str = "auction-postgres-rebuild-preflight.v1";

pub(super) const MAINTENANCE_DATABASE_NAME: &str = "postgres";
pub(super) const SUPPORTED_POSTGRES_MAJOR: i32 = 18;

/// 连接集群维护库后收集的重建预检证据。
#[derive(Debug, Clone, Serialize)]
pub struct PostgresRebuildPreflight {
    pub format: &'static str,
    pub collected_at: DateTime<Utc>,
    pub maintenance_database_name: String,
    pub server_version_num: i32,
    pub target_database_name: String,
    pub target_database_oid: i64,
    pub target_owner: String,
    pub target_encoding: String,
    pub target_locale_provider: String,
    pub target_tablespace: String,
    pub target_connection_limit: i32,
    pub target_allows_connections: bool,
    pub target_is_template: bool,
    pub target_has_custom_acl: bool,
    pub target_has_role_settings: bool,
    pub current_user: String,
    pub current_user_superuser: bool,
    pub current_user_createdb: bool,
    pub current_user_owns_target: bool,
    pub current_user_can_create_in_tablespace: bool,
    pub in_recovery: bool,
    pub default_transaction_read_only: bool,
    pub active_target_sessions: i64,
    pub prepared_transactions: i64,
    pub active_logical_replication_slots: i64,
    pub subscriptions: i64,
    pub advisory_lock_acquired: bool,
    pub rebuild_permitted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PostgresRebuildResult {
    pub database_name: String,
    pub database_owner: String,
    pub previous_database_oid: i64,
    pub new_database_oid: i64,
    pub encoding: String,
    pub locale_provider: String,
    pub tablespace: String,
    pub connection_limit: i32,
    pub template_database: &'static str,
    pub forced_drop: bool,
}

/// PostgreSQL 重建边界的安全错误分类。
#[derive(Debug, Error)]
pub enum PostgresRebuildError {
    #[error("confirmed PostgreSQL rebuild database name is invalid")]
    InvalidDatabaseName,

    #[error("refusing to rebuild a protected PostgreSQL maintenance database")]
    ProtectedDatabase,

    #[error("failed to connect to the PostgreSQL maintenance database")]
    Connect(#[source] sqlx::Error),

    #[error("failed to query PostgreSQL rebuild state at {stage}")]
    Query {
        stage: &'static str,
        #[source]
        source: sqlx::Error,
    },

    #[error("PostgreSQL rebuild preflight failed: {reason}")]
    UnsafePreflight { reason: &'static str },

    #[error(
        "PostgreSQL rebuild mutation failed at {stage}; database_dropped={database_dropped:?} database_recreated={database_recreated:?}; None means outcome unknown"
    )]
    Mutation {
        stage: &'static str,
        /// None 表示命令已开始但未取得完整响应，不能据连接错误推断数据库未变更。
        database_dropped: Option<bool>,
        /// None 表示 CREATE 的远端结果未知；Some(false) 只用于尚未执行 CREATE。
        database_recreated: Option<bool>,
        #[source]
        source: sqlx::Error,
    },

    #[error(
        "PostgreSQL rebuild postcondition failed at {stage}; database_dropped=true database_recreated=true"
    )]
    Postcondition { stage: &'static str },
}

impl PostgresRebuildError {
    /// 返回已知或未知的破坏性状态；结果未知也必须发布失败回执，不能当成未变更。
    pub const fn mutation_state(&self) -> Option<(&'static str, Option<bool>, Option<bool>)> {
        match self {
            Self::Mutation { stage, database_dropped, database_recreated, .. } => {
                Some((stage, *database_dropped, *database_recreated))
            }
            Self::Postcondition { stage } => Some((stage, Some(true), Some(true))),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TargetDatabaseDefinition {
    pub(super) encoding: String,
    pub(super) locale_provider: LocaleProvider,
    pub(super) lc_collate: String,
    pub(super) lc_ctype: String,
    pub(super) provider_locale: Option<String>,
    pub(super) icu_rules: Option<String>,
    pub(super) tablespace: String,
    pub(super) connection_limit: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LocaleProvider {
    Builtin,
    Libc,
    Icu,
}

impl LocaleProvider {
    pub(super) fn from_catalog(value: &str) -> Result<Self, PostgresRebuildError> {
        match value {
            "b" => Ok(Self::Builtin),
            "c" => Ok(Self::Libc),
            "i" => Ok(Self::Icu),
            _ => Err(PostgresRebuildError::UnsafePreflight {
                reason: "target database uses an unsupported locale provider",
            }),
        }
    }

    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Libc => "libc",
            Self::Icu => "icu",
        }
    }
}

pub(super) struct CollectedState {
    pub(super) preflight: PostgresRebuildPreflight,
    pub(super) definition: TargetDatabaseDefinition,
}
