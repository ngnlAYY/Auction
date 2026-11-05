//! 保留清理只读演练的对外模型；只描述候选规模，不代表归档、删除或资格授权。

use chrono::{DateTime, Utc};
use serde::Serialize;
use thiserror::Error;

/// 只读演练回执的机器可读格式标识。
pub const RETENTION_DRY_RUN_FORMAT: &str = "auction-retention-dry-run.v1";

/// 一个候选规则的期限身份；期限来自政策 v2，`cutoff` 是本次快照时间减去期限。
#[derive(Debug, Serialize)]
pub struct RetentionRuleDryRun {
    pub rule_id: String,
    pub table: Option<String>,
    pub duration_days: i64,
    pub cutoff: DateTime<Utc>,
}

/// 单个数据库表的候选计数；规则候选互斥，候选与保留相加等于总行数。
#[derive(Debug, Serialize)]
pub struct RetentionTableDryRun {
    pub table: String,
    pub rule_ids: Vec<String>,
    pub total_rows: i64,
    pub candidate_rows: i64,
    pub retained_rows: i64,
    /// 候选行中仍被外键引用阻断、不能删除的行数（当前只有 realtime_events）。
    pub blocked_rows: i64,
    /// 显式排除的有效会话数（含永久会话）。
    pub excluded_active_rows: i64,
    /// 已过期但仍在 30 天宽限期内的会话数；不是候选。
    pub grace_rows: i64,
    /// 未解决、因此永久保留的失败记录数。
    pub unresolved_rows: i64,
}

#[derive(Debug, Serialize)]
pub struct RetentionFamilyDryRun {
    pub dataset_family: String,
    pub feature_owner: String,
    pub policy_state: String,
    pub retention_kind: String,
    pub rules: Vec<RetentionRuleDryRun>,
    pub tables: Vec<RetentionTableDryRun>,
    pub candidate_rows: i64,
    pub retained_rows: i64,
    pub blocked_rows: i64,
    pub excluded_active_rows: i64,
}

/// 只读演练报告；`mutation_executed` 与 `deletion_authorized` 恒为 false。
#[derive(Debug, Serialize)]
pub struct RetentionDryRunReport {
    pub format: &'static str,
    pub run_id: uuid::Uuid,
    pub collected_at: DateTime<Utc>,
    pub database_name: String,
    pub server_version: String,
    pub transaction_snapshot: String,
    pub transaction_read_only: String,
    pub policy_id: String,
    pub policy_version: i64,
    pub policy_sha256: String,
    pub deletion_requires_dry_run_and_operator_approval: bool,
    pub families: Vec<RetentionFamilyDryRun>,
    pub candidate_rows: i64,
    pub retained_rows: i64,
    pub mutation_executed: bool,
    pub deletion_authorized: bool,
    pub notes: Vec<String>,
}

/// 只读演练失败的安全分类；不包含 SQL 文本或行内容。
#[derive(Debug, Error)]
pub enum RetentionDryRunError {
    #[error("retention dry-run id is missing")]
    InvalidRunId,

    #[error("confirmed PostgreSQL database name is invalid")]
    InvalidDatabaseName,

    #[error("confirmed PostgreSQL database does not match the connected database")]
    DatabaseMismatch { expected: String, actual: String },

    #[error("Auction migration history is not present")]
    MigrationHistoryMissing,

    #[error("Auction migration history is not the current complete prefix")]
    MigrationNotCurrent,

    #[error("versioned retention policy is missing, invalid or not v2")]
    PolicyInvalid,

    #[error("retention dry-run transaction is not read-only")]
    ReadOnlyTransactionInvalid,

    #[error("retention dry-run count exceeds the representable range")]
    CountOverflow,

    #[error("failed to query PostgreSQL retention dry-run counts at {stage}")]
    Query {
        stage: &'static str,
        #[source]
        source: sqlx::Error,
    },

    #[error("failed to finish PostgreSQL retention dry-run transaction")]
    Commit {
        #[source]
        source: sqlx::Error,
    },
}
