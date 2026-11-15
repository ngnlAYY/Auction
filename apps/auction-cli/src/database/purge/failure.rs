//! 文件已删除而数据库执行返回错误时的本地失败回执与错误分类。

use std::path::Path;

use anyhow::Context;
use auction_storage::{
    RETENTION_PURGE_RECEIPT_FORMAT, RetentionPurgeError, RetentionPurgeFilesystemResult,
};
use chrono::Utc;

use super::dry_run::DryRunReceipt;
use crate::support::files;

/// 数据库执行返回错误。文件族已删除文件时，先写一份 `outcome=failed` 的本地回执记录实际文件
/// 结果与错误分类再返回：文件删除不可撤销，不能只留一行错误输出。回执本身写不进去时把两段
/// 原因一起返回。
pub(super) fn execution_failed(
    dry_run_receipt: &Path,
    receipt: &DryRunReceipt,
    receipt_path: &Path,
    filesystem: Option<RetentionPurgeFilesystemResult>,
    error: RetentionPurgeError,
) -> anyhow::Error {
    let failure_code = failure_code(&error);
    let error = anyhow::Error::new(error).context("执行保留清理失败");
    let Some(filesystem) = filesystem.filter(|result| result.deleted_files > 0) else {
        return error;
    };
    let deleted_files = filesystem.deleted_files;
    let document = serde_json::json!({
        "format": RETENTION_PURGE_RECEIPT_FORMAT,
        "created_at": Utc::now(),
        "outcome": "failed",
        "failure_code": failure_code,
        "dry_run_receipt": dry_run_receipt,
        "dry_run_run_id": receipt.run_id,
        "dry_run_created_at": receipt.created_at,
        "database": receipt.database_retention.database_name,
        "execution": null,
        "filesystem": filesystem,
        "mutation_executed": true,
        "notes": [
            "加密诊断文件已按批准上限删除，随后数据库执行返回错误；没有数据库执行报告与 auction.retention.purge 审计",
            "预校验类错误（如回执过期）不删除任何数据库行；执行期错误可能已有批次提交，须人工核对",
            "重新执行 db retention-dry-run 并逐族重新批准后再清理",
        ],
    });
    let written = serde_json::to_vec_pretty(&document)
        .context("序列化保留清理失败回执失败")
        .and_then(|bytes| files::write_private(receipt_path, &bytes));
    match written {
        Ok(()) => error.context(format!(
            "已删除 {deleted_files} 个加密诊断文件，数据库清理未完成；失败回执已写入 {}",
            receipt_path.display()
        )),
        Err(write_error) => anyhow::anyhow!(
            "{error:#}；已删除 {deleted_files} 个加密诊断文件，失败回执也未写入：{write_error:#}"
        ),
    }
}

/// 数据库执行错误的固定分类；回执只记录它，不写驱动原文。
fn failure_code(error: &RetentionPurgeError) -> &'static str {
    match error {
        RetentionPurgeError::InvalidRequest => "invalid_request",
        RetentionPurgeError::InvalidRunId => "invalid_run_id",
        RetentionPurgeError::InvalidBatchSize => "invalid_batch_size",
        RetentionPurgeError::InvalidDatabaseName => "invalid_database_name",
        RetentionPurgeError::DatabaseMismatch { .. } => "database_mismatch",
        RetentionPurgeError::ReceiptStale => "receipt_stale",
        RetentionPurgeError::ReceiptInFuture => "receipt_in_future",
        RetentionPurgeError::PolicyInvalid => "policy_invalid",
        RetentionPurgeError::PolicyMismatch => "policy_mismatch",
        RetentionPurgeError::MigrationNotCurrent => "migration_not_current",
        RetentionPurgeError::UnknownFamily => "unknown_family",
        RetentionPurgeError::RetainForeverFamily => "retain_forever_family",
        RetentionPurgeError::ApprovalInvalid => "approval_invalid",
        RetentionPurgeError::ApprovalTokenMismatch => "approval_token_mismatch",
        RetentionPurgeError::TableNotPurgeable => "table_not_purgeable",
        RetentionPurgeError::CandidateCountInvalid => "candidate_count_invalid",
        RetentionPurgeError::FilesystemInvalid => "filesystem_invalid",
        RetentionPurgeError::CountOverflow => "count_overflow",
        RetentionPurgeError::Query { .. } => "query_failed",
        RetentionPurgeError::Commit { .. } => "commit_failed",
    }
}
