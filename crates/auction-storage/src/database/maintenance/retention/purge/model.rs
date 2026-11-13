//! 保留清理执行的请求、批准绑定与回执模型。执行只覆盖政策 v2 的代码内闭集运行态表；
//! 批准只能来自一次新鲜的只读演练，并按数据族绑定候选行/字节与表级计数。

use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

/// 执行回执的机器可读格式标识。
pub const RETENTION_PURGE_RECEIPT_FORMAT: &str = "auction-retention-purge-receipt.v1";

/// 逐数据族批准令牌的规范串版本；改变规范串即让旧令牌全部失效。
pub const RETENTION_PURGE_APPROVAL_FORMAT: &str = "auction-retention-purge-approval.v1";

/// 只读演练回执在执行前的最大年龄；过期必须先重新演练。
pub const RETENTION_PURGE_RECEIPT_MAX_AGE_SECONDS: i64 = 900;

/// 演练快照到回执完成的允许时长；超出说明回执时间与快照不配套。
pub const RETENTION_PURGE_DRY_RUN_MAX_DURATION_SECONDS: i64 = 600;

/// 单批删除的默认与允许范围；每批一个独立事务。
pub const RETENTION_PURGE_DEFAULT_BATCH_SIZE: u32 = 500;
pub const RETENTION_PURGE_MIN_BATCH_SIZE: u32 = 1;
pub const RETENTION_PURGE_MAX_BATCH_SIZE: u32 = 5_000;

/// 政策 v2 的版本号；批准必须与编译制品一致。
pub const RETENTION_PURGE_POLICY_VERSION: i64 = 2;

/// 一次批准中的单表上限；计数来自只读演练回执，执行时不得越过。
#[derive(Debug, Clone)]
pub struct RetentionPurgeTableApproval {
    pub table: String,
    pub rule_ids: Vec<String>,
    pub candidate_rows: i64,
}

/// 单个数据族的批准；令牌绑定数据库、演练 run id、政策摘要与精确计数。
#[derive(Debug, Clone)]
pub struct RetentionPurgeApproval {
    pub dataset_family: String,
    pub approval_token: String,
    pub candidate_rows: i64,
    pub candidate_bytes: Option<u64>,
    pub tables: Vec<RetentionPurgeTableApproval>,
}

/// 已执行（或已尝试）失败关闭的文件族清理结果；计数来自实际 unlink 的文件。
#[derive(Debug, Clone, Serialize)]
pub struct RetentionPurgeFilesystemResult {
    pub directory_present: bool,
    pub examined_files: u64,
    pub deleted_files: u64,
    pub deleted_bytes: u64,
    pub retained_files: u64,
    pub write_unknown_held_files: u64,
    pub write_unknown_over_hold_window_files: u64,
    pub unexpected_entries: u64,
    pub limit_reached: bool,
    pub needs_follow_up: bool,
    pub failure_code: Option<String>,
}

/// 一次清理请求；数据库身份、演练身份与批准都必须由调用方从回执解析后原样传入。
#[derive(Debug, Clone)]
pub struct RetentionPurgeRequest {
    pub database_name: String,
    pub dry_run_run_id: Uuid,
    pub dry_run_collected_at: DateTime<Utc>,
    pub dry_run_created_at: DateTime<Utc>,
    pub policy_sha256: String,
    pub batch_size: u32,
    pub approvals: Vec<RetentionPurgeApproval>,
    /// 文件族已在本进程执行的结果；只在批准了 `mail_diagnostic_files` 时存在。
    pub filesystem: Option<RetentionPurgeFilesystemResult>,
}

/// 清理结果的终态；`partial_limit_reached` 表示批准上限内已删完但仍有候选。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionPurgeOutcome {
    Complete,
    PartialLimitReached,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetentionPurgeTableResult {
    pub table: String,
    pub rule_ids: Vec<String>,
    pub approved_rows: i64,
    pub deleted_rows: i64,
    /// 删除时用 `pg_column_size` 计算的行载荷字节数；不是物理文件页面大小。
    pub deleted_row_bytes: i64,
    pub batches: u32,
    /// 删除谓词仍然匹配、但未在本轮批准上限内删除的行。
    pub remaining_candidate_rows: i64,
    /// 按年龄本可删除、但被外键或重放关系阻断的事件行；永不删除。
    pub blocked_candidate_rows: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetentionPurgeFamilyResult {
    pub dataset_family: String,
    pub retention_kind: String,
    pub approved_rows: i64,
    pub approved_bytes: Option<u64>,
    pub deleted_rows: i64,
    pub deleted_bytes: i64,
    pub tables: Vec<RetentionPurgeTableResult>,
    pub outcome: RetentionPurgeOutcome,
}

/// 执行回执；`mutation_executed` 在任一 DELETE 或文件 unlink 成功后才为 true。
#[derive(Debug, Clone, Serialize)]
pub struct RetentionPurgeReport {
    pub format: &'static str,
    pub run_id: Uuid,
    pub dry_run_run_id: Uuid,
    pub database_name: String,
    pub policy_version: i64,
    pub policy_sha256: String,
    pub batch_size: u32,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub outcome: RetentionPurgeOutcome,
    pub failure_code: Option<&'static str>,
    pub families: Vec<RetentionPurgeFamilyResult>,
    pub deleted_rows: i64,
    pub deleted_row_bytes: i64,
    pub filesystem: Option<RetentionPurgeFilesystemResult>,
    pub audit_id: Option<Uuid>,
    pub mutation_executed: bool,
    pub deletion_authorized: bool,
    pub notes: Vec<String>,
}

/// 清理失败的安全分类；预校验失败不产生任何变更，执行失败由回执记录。
#[derive(Debug, Error)]
pub enum RetentionPurgeError {
    #[error("retention purge request is invalid")]
    InvalidRequest,

    #[error("retention purge run id is missing")]
    InvalidRunId,

    #[error("retention purge batch size is outside the allowed range")]
    InvalidBatchSize,

    #[error("confirmed PostgreSQL database name is invalid")]
    InvalidDatabaseName,

    #[error("confirmed PostgreSQL database does not match the connected database")]
    DatabaseMismatch { expected: String, actual: String },

    #[error("retention dry-run receipt is stale")]
    ReceiptStale,

    #[error("retention dry-run receipt time is in the future")]
    ReceiptInFuture,

    #[error("versioned retention policy is missing, invalid or not v2")]
    PolicyInvalid,

    #[error("compiled retention policy does not match the approved receipt")]
    PolicyMismatch,

    #[error("Auction migration history is missing or not the current complete prefix")]
    MigrationNotCurrent,

    #[error("retention family is not part of policy v2")]
    UnknownFamily,

    #[error("retention family is retain-forever and cannot be purged")]
    RetainForeverFamily,

    #[error("retention family approval is missing, duplicated or malformed")]
    ApprovalInvalid,

    #[error("retention approval token does not bind the supplied counts")]
    ApprovalTokenMismatch,

    #[error("retention table is not part of the purge allowlist")]
    TableNotPurgeable,

    #[error("retention approval counts are inconsistent")]
    CandidateCountInvalid,

    #[error("retention filesystem approval is missing or inconsistent")]
    FilesystemInvalid,

    #[error("retention purge count exceeds the representable range")]
    CountOverflow,

    #[error("failed to query PostgreSQL retention purge state at {stage}")]
    Query {
        stage: &'static str,
        #[source]
        source: sqlx::Error,
    },

    #[error("failed to finish PostgreSQL retention purge transaction at {stage}")]
    Commit {
        stage: &'static str,
        #[source]
        source: sqlx::Error,
    },
}

/// 逐数据族批准令牌：SHA-256 的十六进制摘要，绑定演练身份、政策与精确计数。
/// 同一数据库同一演练同一族得到同一令牌；任一字段变化都会得到不同令牌。
pub fn retention_purge_approval_token(
    database_name: &str,
    dry_run_run_id: Uuid,
    policy_sha256: &str,
    dataset_family: &str,
    candidate_rows: i64,
    candidate_bytes: Option<u64>,
    tables: &[(String, i64)],
) -> String {
    let mut tables = tables.to_vec();
    tables.sort();
    let mut canonical = String::new();
    canonical.push_str(RETENTION_PURGE_APPROVAL_FORMAT);
    canonical.push('\n');
    canonical.push_str(&format!("database={database_name}\n"));
    canonical.push_str(&format!("run_id={dry_run_run_id}\n"));
    canonical.push_str(&format!("policy_sha256={policy_sha256}\n"));
    canonical.push_str(&format!("family={dataset_family}\n"));
    canonical.push_str(&format!("candidate_rows={candidate_rows}\n"));
    match candidate_bytes {
        Some(bytes) => canonical.push_str(&format!("candidate_bytes={bytes}\n")),
        None => canonical.push_str("candidate_bytes=none\n"),
    }
    for (table, rows) in &tables {
        canonical.push_str(&format!("table={table}:{rows}\n"));
    }
    auction_application::core::lower_hex(&Sha256::digest(canonical.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::{RETENTION_PURGE_APPROVAL_FORMAT, retention_purge_approval_token};
    use uuid::Uuid;

    fn token(family: &str, rows: i64) -> String {
        retention_purge_approval_token(
            "auction_test",
            Uuid::nil(),
            &"a".repeat(64),
            family,
            rows,
            None,
            &[("service_heartbeats".to_owned(), rows)],
        )
    }

    #[test]
    fn approval_token_is_deterministic_and_table_order_independent() {
        let first = retention_purge_approval_token(
            "auction_test",
            Uuid::nil(),
            &"b".repeat(64),
            "session_records",
            7,
            None,
            &[("public_sessions".to_owned(), 3), ("admin_sessions".to_owned(), 4)],
        );
        let second = retention_purge_approval_token(
            "auction_test",
            Uuid::nil(),
            &"b".repeat(64),
            "session_records",
            7,
            None,
            &[("admin_sessions".to_owned(), 4), ("public_sessions".to_owned(), 3)],
        );
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|value| value.is_ascii_hexdigit()));
        assert!(first.chars().all(|value| !value.is_ascii_uppercase()));
    }

    #[test]
    fn approval_token_changes_with_every_bound_field() {
        let base = token("service_heartbeat", 3);
        assert_ne!(base, token("service_heartbeat", 4));
        assert_ne!(base, token("realtime_events", 3));
        assert_ne!(
            base,
            retention_purge_approval_token(
                "other_database",
                Uuid::nil(),
                &"a".repeat(64),
                "service_heartbeat",
                3,
                None,
                &[("service_heartbeats".to_owned(), 3)],
            )
        );
        assert_ne!(
            base,
            retention_purge_approval_token(
                "auction_test",
                Uuid::from_u128(1),
                &"a".repeat(64),
                "service_heartbeat",
                3,
                None,
                &[("service_heartbeats".to_owned(), 3)],
            )
        );
        assert_ne!(
            base,
            retention_purge_approval_token(
                "auction_test",
                Uuid::nil(),
                &"c".repeat(64),
                "service_heartbeat",
                3,
                None,
                &[("service_heartbeats".to_owned(), 3)],
            )
        );
        assert_ne!(
            base,
            retention_purge_approval_token(
                "auction_test",
                Uuid::nil(),
                &"a".repeat(64),
                "service_heartbeat",
                3,
                Some(1),
                &[("service_heartbeats".to_owned(), 3)],
            )
        );
    }

    #[test]
    fn approval_token_canonical_prefix_is_versioned() {
        assert_eq!(RETENTION_PURGE_APPROVAL_FORMAT, "auction-retention-purge-approval.v1");
    }
}
