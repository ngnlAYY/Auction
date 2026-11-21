//! 保留清理的只读演练回执：有界读取 owner-only 文件、核对格式/身份/政策/新鲜度，以及删除
//! 文件前的剩余有效期门禁。

use std::{fs, io::Read, os::unix::fs::MetadataExt, path::Path};

use anyhow::Context;
use auction_storage::{RETENTION_DRY_RUN_FORMAT, RETENTION_PURGE_RECEIPT_MAX_AGE_SECONDS};
use auction_time::{TimeDelta, Timestamp};
use serde::Deserialize;
use uuid::Uuid;

use crate::support::{digest::is_lower_hex, files};

const MAX_RECEIPT_BYTES: u64 = 4 * 1024 * 1024;
/// 删除文件前演练回执至少还剩的有效期（秒）。文件删除之后数据库执行会再校验一次回执新鲜度，
/// 余量不足时文件已删而数据库执行因回执过期被拒。
pub(super) const FILESYSTEM_PURGE_MIN_HEADROOM_SECONDS: i64 = 60;

#[derive(Debug, Deserialize)]
pub(super) struct DryRunReceipt {
    pub(super) format: String,
    pub(super) run_id: Uuid,
    pub(super) created_at: Timestamp,
    pub(super) database_retention: DatabaseRetention,
    pub(super) filesystem_retention: FilesystemRetention,
}

#[derive(Debug, Deserialize)]
pub(super) struct DatabaseRetention {
    pub(super) format: String,
    pub(super) run_id: Uuid,
    pub(super) collected_at: Timestamp,
    pub(super) database_name: String,
    pub(super) policy_version: i64,
    pub(super) policy_sha256: String,
    pub(super) transaction_read_only: String,
    pub(super) mutation_executed: bool,
    pub(super) deletion_authorized: bool,
    pub(super) families: Vec<DryRunFamily>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DryRunFamily {
    pub(super) dataset_family: String,
    pub(super) retention_kind: String,
    pub(super) tables: Vec<DryRunTable>,
    pub(super) candidate_rows: i64,
}

#[derive(Debug, Deserialize)]
pub(super) struct DryRunTable {
    pub(super) table: String,
    pub(super) rule_ids: Vec<String>,
    pub(super) candidate_rows: i64,
}

#[derive(Debug, Deserialize)]
pub(super) struct FilesystemRetention {
    pub(super) dataset_family: String,
    pub(super) candidate_files: u64,
    pub(super) root_identity_sha256: String,
    pub(super) candidate_bytes: u64,
}

pub(super) fn load_receipt(path: &Path) -> anyhow::Result<DryRunReceipt> {
    anyhow::ensure!(path.is_absolute(), "只读演练回执必须是绝对路径：{}", path.display());
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("读取只读演练回执失败：{}", path.display()))?;
    anyhow::ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == files::effective_uid()
            && metadata.mode() & 0o077 == 0
            && metadata.nlink() == 1,
        "只读演练回执必须是当前账号拥有的单链接 owner-only 普通文件：{}",
        path.display()
    );
    anyhow::ensure!(
        metadata.len() <= MAX_RECEIPT_BYTES,
        "只读演练回执超过 {} 字节：{}",
        MAX_RECEIPT_BYTES,
        path.display()
    );
    let mut bytes = Vec::new();
    files::open_regular(path)?
        .take(MAX_RECEIPT_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("读取只读演练回执内容失败")?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_RECEIPT_BYTES,
        "只读演练回执超过 {} 字节：{}",
        MAX_RECEIPT_BYTES,
        path.display()
    );
    serde_json::from_slice(&bytes).context("解析只读演练回执失败")
}

pub(super) fn validate_receipt(
    receipt: &DryRunReceipt,
    database: &str,
    policy_version: i64,
    policy_sha256: &str,
    now: Timestamp,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        receipt.format == "auction-retention-dry-run-receipt.v2",
        "只读演练回执格式不受支持"
    );
    let database_retention = &receipt.database_retention;
    anyhow::ensure!(
        database_retention.format == RETENTION_DRY_RUN_FORMAT,
        "只读演练回执缺少数据库报告"
    );
    anyhow::ensure!(receipt.run_id == database_retention.run_id, "只读演练回执 run id 不一致");
    anyhow::ensure!(
        database_retention.database_name == database,
        "只读演练回执数据库 {} 与 --database {} 不一致",
        database_retention.database_name,
        database
    );
    anyhow::ensure!(
        database_retention.policy_version == policy_version
            && database_retention.policy_sha256 == policy_sha256,
        "只读演练回执的政策身份与编译制品不一致；先重新演练"
    );
    anyhow::ensure!(
        is_lower_hex(&receipt.filesystem_retention.root_identity_sha256, 64),
        "只读演练回执缺少有效的文件系统根身份摘要"
    );
    anyhow::ensure!(
        database_retention.transaction_read_only == "on"
            && !database_retention.mutation_executed
            && !database_retention.deletion_authorized,
        "只读演练回执不是未变更的只读快照"
    );
    anyhow::ensure!(
        receipt.created_at >= database_retention.collected_at
            && receipt.created_at - database_retention.collected_at
                <= auction_time::TimeDelta::seconds(
                    auction_storage::RETENTION_PURGE_DRY_RUN_MAX_DURATION_SECONDS
                ),
        "只读演练回执时间与快照时间不配套"
    );
    anyhow::ensure!(
        receipt.created_at <= now + auction_time::TimeDelta::minutes(2),
        "只读演练回执时间在未来"
    );
    anyhow::ensure!(
        now - receipt.created_at
            <= auction_time::TimeDelta::seconds(
                auction_storage::RETENTION_PURGE_RECEIPT_MAX_AGE_SECONDS
            ),
        "只读演练回执已过期；先重新执行 db retention-dry-run"
    );
    Ok(())
}

/// 删除文件前的回执余量门禁：到期时刻（`created_at` 加最大年龄）距 `now` 不足
/// [`FILESYSTEM_PURGE_MIN_HEADROOM_SECONDS`] 秒即拒绝，此时尚未删除任何文件。
pub(super) fn require_receipt_headroom(
    created_at: Timestamp,
    now: Timestamp,
) -> anyhow::Result<()> {
    let remaining = created_at + TimeDelta::seconds(RETENTION_PURGE_RECEIPT_MAX_AGE_SECONDS) - now;
    anyhow::ensure!(
        remaining >= TimeDelta::seconds(FILESYSTEM_PURGE_MIN_HEADROOM_SECONDS),
        "只读演练回执只剩 {} 秒有效期，不足删除文件所需的 {FILESYSTEM_PURGE_MIN_HEADROOM_SECONDS} 秒\
         余量；未删除任何文件或数据库行，先重新执行 db retention-dry-run 并重新批准",
        remaining.num_seconds().max(0)
    );
    Ok(())
}
