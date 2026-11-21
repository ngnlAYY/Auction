//! 保留清理执行的请求、批准绑定与回执模型。执行只覆盖政策 v2 的代码内闭集运行态表；
//! 批准只能来自一次新鲜的只读演练，并按数据族绑定候选行/字节与表级计数。

use std::path::PathBuf;

use auction_time::Timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

/// 执行回执的机器可读格式标识。
pub const RETENTION_PURGE_RECEIPT_FORMAT: &str = "auction-retention-purge-receipt.v1";

/// 逐数据族批准令牌的规范串版本；改变规范串即让旧令牌全部失效。
pub const RETENTION_PURGE_APPROVAL_FORMAT: &str = "auction-retention-purge-approval.v2";

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
    pub filesystem_root_identity_sha256: Option<String>,
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
    pub artifact_sha256: String,
}

/// CLI-produced file purge evidence reference; outcome values stay inside the artifact bytes.
#[derive(Debug, Clone)]
pub struct RetentionPurgeFilesystemArtifactRef {
    pub path: PathBuf,
    pub sha256: [u8; 32],
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RetentionPurgeFilesystemArtifact {
    pub(super) format: String,
    pub(super) run_id: Uuid,
    pub(super) dry_run_run_id: Uuid,
    pub(super) database_name: String,
    pub(super) dataset_family: String,
    pub(super) approval_token_sha256: String,
    pub(super) policy_version: i64,
    pub(super) policy_sha256: String,
    pub(super) filesystem_root_identity_sha256: String,
    pub(super) approved_max_files: u64,
    pub(super) completed_at: Timestamp,
    pub(super) outcome: RetentionPurgeOutcome,
    pub(super) directory_present: bool,
    pub(super) examined_files: u64,
    pub(super) deleted_files: u64,
    pub(super) deleted_bytes: u64,
    pub(super) retained_files: u64,
    pub(super) write_unknown_held_files: u64,
    pub(super) write_unknown_over_hold_window_files: u64,
    pub(super) unexpected_entries: u64,
    pub(super) limit_reached: bool,
    pub(super) needs_follow_up: bool,
    pub(super) failure_code: Option<String>,
}

impl RetentionPurgeFilesystemArtifact {
    pub(super) fn into_result(self, artifact_sha256: String) -> RetentionPurgeFilesystemResult {
        RetentionPurgeFilesystemResult {
            directory_present: self.directory_present,
            examined_files: self.examined_files,
            deleted_files: self.deleted_files,
            deleted_bytes: self.deleted_bytes,
            retained_files: self.retained_files,
            write_unknown_held_files: self.write_unknown_held_files,
            write_unknown_over_hold_window_files: self.write_unknown_over_hold_window_files,
            unexpected_entries: self.unexpected_entries,
            limit_reached: self.limit_reached,
            needs_follow_up: self.needs_follow_up,
            failure_code: self.failure_code,
            artifact_sha256,
        }
    }
}

/// 一次清理请求；数据库身份、演练身份与批准都必须由调用方从回执解析后原样传入。
#[derive(Debug, Clone)]
pub struct RetentionPurgeRequest {
    pub database_name: String,
    pub dry_run_run_id: Uuid,
    pub dry_run_collected_at: Timestamp,
    pub dry_run_created_at: Timestamp,
    pub policy_sha256: String,
    pub batch_size: u32,
    pub approvals: Vec<RetentionPurgeApproval>,
    pub filesystem_artifact: Option<RetentionPurgeFilesystemArtifactRef>,
    /// CLI 主机操作者身份；只并入审计 metadata（`entrypoint`、`os_uid`、可选 `sudo_user`），
    /// 不进入回执，也不改变审计的 actor 与 source。
    pub operator_identity: Option<auction_application::core::CliOperatorIdentity>,
}

/// 清理结果的终态；`partial_limit_reached` 表示批准上限内已删完但仍有候选。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
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

    #[error("retention filesystem artifact has already been consumed")]
    FilesystemArtifactReplay,

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

/// 逐数据族批准令牌的规范化输入。
#[derive(Debug, Clone, Copy)]
pub struct RetentionPurgeApprovalTokenInput<'a> {
    /// 数据库名称。
    pub database_name: &'a str,
    /// 只读演练的运行标识。
    pub dry_run_run_id: Uuid,
    /// 保留政策摘要。
    pub policy_sha256: &'a str,
    /// 数据族名称。
    pub dataset_family: &'a str,
    /// 文件族根目录身份摘要。
    pub filesystem_root_identity_sha256: Option<&'a str>,
    /// 批准的候选行数。
    pub candidate_rows: i64,
    /// 批准的候选字节数。
    pub candidate_bytes: Option<u64>,
    /// 按表绑定的候选行数。
    pub tables: &'a [(String, i64)],
}

/// 逐数据族批准令牌：SHA-256 的十六进制摘要，绑定演练身份、政策与精确计数。
/// 同一数据库同一演练同一族得到同一令牌；任一字段变化都会得到不同令牌。
pub fn retention_purge_approval_token(input: RetentionPurgeApprovalTokenInput<'_>) -> String {
    let mut tables = input.tables.to_vec();
    tables.sort();
    let mut canonical = String::new();
    canonical.push_str(RETENTION_PURGE_APPROVAL_FORMAT);
    canonical.push('\n');
    append_canonical_field(&mut canonical, input.database_name);
    append_canonical_field(&mut canonical, &input.dry_run_run_id.to_string());
    append_canonical_field(&mut canonical, input.policy_sha256);
    append_canonical_field(&mut canonical, input.dataset_family);
    append_canonical_field(&mut canonical, input.filesystem_root_identity_sha256.unwrap_or("none"));
    append_canonical_field(&mut canonical, &input.candidate_rows.to_string());
    append_canonical_field(
        &mut canonical,
        &input.candidate_bytes.map_or_else(|| "none".to_owned(), |bytes| bytes.to_string()),
    );
    append_canonical_field(&mut canonical, &tables.len().to_string());
    for (table, rows) in &tables {
        append_canonical_field(&mut canonical, table);
        append_canonical_field(&mut canonical, &rows.to_string());
    }
    auction_application::core::lower_hex(&Sha256::digest(canonical.as_bytes()))
}

fn append_canonical_field(canonical: &mut String, value: &str) {
    canonical.push_str(&value.len().to_string());
    canonical.push(':');
    canonical.push_str(value);
    canonical.push('\n');
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{
        RETENTION_PURGE_APPROVAL_FORMAT, RetentionPurgeApprovalTokenInput,
        retention_purge_approval_token,
    };

    fn token(family: &str, rows: i64) -> String {
        retention_purge_approval_token(RetentionPurgeApprovalTokenInput {
            database_name: "auction_test",
            dry_run_run_id: Uuid::nil(),
            policy_sha256: &"a".repeat(64),
            dataset_family: family,
            filesystem_root_identity_sha256: None,
            candidate_rows: rows,
            candidate_bytes: None,
            tables: &[("service_heartbeats".to_owned(), rows)],
        })
    }

    #[test]
    fn approval_token_is_deterministic_and_table_order_independent() {
        let first = retention_purge_approval_token(RetentionPurgeApprovalTokenInput {
            database_name: "auction_test",
            dry_run_run_id: Uuid::nil(),
            policy_sha256: &"b".repeat(64),
            dataset_family: "session_records",
            filesystem_root_identity_sha256: None,
            candidate_rows: 7,
            candidate_bytes: None,
            tables: &[("public_sessions".to_owned(), 3), ("admin_sessions".to_owned(), 4)],
        });
        let second = retention_purge_approval_token(RetentionPurgeApprovalTokenInput {
            database_name: "auction_test",
            dry_run_run_id: Uuid::nil(),
            policy_sha256: &"b".repeat(64),
            dataset_family: "session_records",
            filesystem_root_identity_sha256: None,
            candidate_rows: 7,
            candidate_bytes: None,
            tables: &[("admin_sessions".to_owned(), 4), ("public_sessions".to_owned(), 3)],
        });
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|value| value.is_ascii_hexdigit()));
        assert!(first.chars().all(|value| !value.is_ascii_uppercase()));
    }

    #[test]
    fn approval_token_changes_with_every_bound_field() {
        let policy_sha256 = "a".repeat(64);
        let tables = [("service_heartbeats".to_owned(), 3)];
        let base_input = RetentionPurgeApprovalTokenInput {
            database_name: "auction_test",
            dry_run_run_id: Uuid::nil(),
            policy_sha256: &policy_sha256,
            dataset_family: "service_heartbeat",
            filesystem_root_identity_sha256: None,
            candidate_rows: 3,
            candidate_bytes: None,
            tables: &tables,
        };
        let base = retention_purge_approval_token(base_input);
        assert_ne!(base, token("service_heartbeat", 4));
        assert_ne!(base, token("realtime_events", 3));
        assert_ne!(
            base,
            retention_purge_approval_token(RetentionPurgeApprovalTokenInput {
                database_name: "other_database",
                ..base_input
            })
        );
        assert_ne!(
            base,
            retention_purge_approval_token(RetentionPurgeApprovalTokenInput {
                dry_run_run_id: Uuid::from_u128(1),
                ..base_input
            })
        );
        let other_policy_sha256 = "c".repeat(64);
        assert_ne!(
            base,
            retention_purge_approval_token(RetentionPurgeApprovalTokenInput {
                policy_sha256: &other_policy_sha256,
                ..base_input
            })
        );
        let root_identity_sha256 = "d".repeat(64);
        assert_ne!(
            base,
            retention_purge_approval_token(RetentionPurgeApprovalTokenInput {
                filesystem_root_identity_sha256: Some(&root_identity_sha256),
                ..base_input
            })
        );
        assert_ne!(
            base,
            retention_purge_approval_token(RetentionPurgeApprovalTokenInput {
                candidate_bytes: Some(1),
                ..base_input
            })
        );
    }

    #[test]
    fn approval_token_canonical_prefix_is_versioned() {
        assert_eq!(RETENTION_PURGE_APPROVAL_FORMAT, "auction-retention-purge-approval.v2");
    }
}
