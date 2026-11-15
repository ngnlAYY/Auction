//! 保留清理的回执余量门禁与失败回执回归用例；不连接数据库、不删除真实诊断文件。

use std::{fs, os::unix::fs::PermissionsExt, path::Path, path::PathBuf, time::Duration};

use auction_storage::{
    RETENTION_PURGE_RECEIPT_MAX_AGE_SECONDS, RetentionPurgeError, RetentionPurgeFilesystemResult,
};
use chrono::{DateTime, TimeDelta, Utc};
use uuid::Uuid;

use super::dry_run::{DryRunReceipt, require_receipt_headroom};
use super::failure::execution_failed;
use super::zero_filesystem_result;
use crate::database::{days_to_duration, mail_diagnostic_windows};

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("auction-retention-purge-{}", Uuid::new_v4()));
        fs::create_dir(&path).expect("建立临时目录");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("设置权限");
        Self(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn dry_run_receipt(created_at: DateTime<Utc>) -> DryRunReceipt {
    let run_id = Uuid::new_v4();
    serde_json::from_value(serde_json::json!({
        "format": "auction-retention-dry-run-receipt.v1",
        "run_id": run_id,
        "created_at": created_at,
        "database_retention": {
            "format": auction_storage::RETENTION_DRY_RUN_FORMAT,
            "run_id": run_id,
            "collected_at": created_at,
            "database_name": "auction",
            "policy_version": 2,
            "policy_sha256": "a".repeat(64),
            "transaction_read_only": "on",
            "mutation_executed": false,
            "deletion_authorized": false,
            "families": [],
        },
        "filesystem_retention": {
            "dataset_family": "mail_diagnostic_files",
            "candidate_files": 3,
            "candidate_bytes": 300,
        },
    }))
    .expect("演练回执夹具")
}

fn deleted(files: u64) -> RetentionPurgeFilesystemResult {
    RetentionPurgeFilesystemResult {
        directory_present: true,
        examined_files: 3,
        deleted_files: files,
        deleted_bytes: files * 100,
        ..zero_filesystem_result()
    }
}

/// 剩余有效期按 `created_at + 最大年龄 - now` 计算，恰好 60 秒放行，少一秒就拒绝。
#[test]
fn headroom_requires_sixty_seconds_before_expiry() {
    let now = Utc::now();
    let created = |remaining: i64| {
        now - TimeDelta::seconds(RETENTION_PURGE_RECEIPT_MAX_AGE_SECONDS)
            + TimeDelta::seconds(remaining)
    };
    require_receipt_headroom(created(61), now).expect("余量充足");
    require_receipt_headroom(created(60), now).expect("恰好 60 秒放行");
    let error = require_receipt_headroom(created(59), now).expect_err("不足 60 秒必须拒绝");
    let text = format!("{error:#}");
    assert!(text.contains("59 秒") && text.contains("db retention-dry-run"), "{text}");
    let expired = require_receipt_headroom(created(-30), now).expect_err("已过期必须拒绝");
    assert!(format!("{expired:#}").contains("只剩 0 秒"));
}

/// 只读演练与实际清理共用的诊断文件期限来自内嵌政策 v2；天数换算拒绝负数、极大值饱和。
#[test]
fn diagnostic_windows_come_from_the_embedded_policy() {
    assert_eq!(mail_diagnostic_windows().expect("政策 v2 含两条诊断期限"), (7, 90));
    assert_eq!(days_to_duration(7).expect("正数天"), Duration::from_secs(7 * 86_400));
    assert_eq!(days_to_duration(0).expect("零天"), Duration::ZERO);
    assert!(days_to_duration(-1).is_err(), "负数期限必须失败关闭");
    assert_eq!(days_to_duration(i64::MAX).expect("极大值饱和"), Duration::from_secs(u64::MAX));
}

/// 文件已删而数据库执行返回错误：先写 outcome=failed 的本地回执（文件结果与固定错误码），
/// 错误同时说明删除了多少文件与回执位置。
#[test]
fn failed_receipt_is_written_when_files_were_deleted() {
    let directory = TempDirectory::new();
    let receipt_path = directory.0.join("purge.json");
    let receipt = dry_run_receipt(Utc::now());
    let error = execution_failed(
        Path::new("/var/backups/auction/dry-run.json"),
        &receipt,
        &receipt_path,
        Some(deleted(2)),
        RetentionPurgeError::ReceiptStale,
    );
    let text = format!("{error:#}");
    assert!(text.contains("已删除 2 个加密诊断文件") && text.contains("失败回执已写入"), "{text}");
    assert!(text.contains("执行保留清理失败"), "{text}");

    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(&receipt_path).expect("失败回执")).expect("JSON");
    assert_eq!(document["format"], auction_storage::RETENTION_PURGE_RECEIPT_FORMAT);
    assert_eq!(document["outcome"], "failed");
    assert_eq!(document["failure_code"], "receipt_stale");
    assert_eq!(document["filesystem"]["deleted_files"], 2);
    assert_eq!(document["mutation_executed"], true);
    assert!(document["execution"].is_null());
    assert_eq!(document["dry_run_run_id"], receipt.run_id.to_string());
}

/// 没有删除任何文件时不写回执，原错误原样返回（与预校验失败同一口径）。
#[test]
fn no_receipt_without_deleted_files() {
    let directory = TempDirectory::new();
    let receipt_path = directory.0.join("purge.json");
    let receipt = dry_run_receipt(Utc::now());
    for filesystem in [None, Some(deleted(0))] {
        let error = execution_failed(
            Path::new("/var/backups/auction/dry-run.json"),
            &receipt,
            &receipt_path,
            filesystem,
            RetentionPurgeError::MigrationNotCurrent,
        );
        assert!(format!("{error:#}").starts_with("执行保留清理失败"));
        assert!(!receipt_path.exists());
    }
}

/// 回执写不进去（目标已存在）时两段原因一起返回，不覆盖已有文件。
#[test]
fn receipt_write_failure_is_reported_with_the_original_error() {
    let directory = TempDirectory::new();
    let receipt_path = directory.0.join("purge.json");
    fs::write(&receipt_path, b"existing").expect("预置文件");
    let error = execution_failed(
        Path::new("/var/backups/auction/dry-run.json"),
        &dry_run_receipt(Utc::now()),
        &receipt_path,
        Some(deleted(1)),
        RetentionPurgeError::ReceiptStale,
    );
    let text = format!("{error:#}");
    assert!(text.contains("失败回执也未写入") && text.contains("receipt is stale"), "{text}");
    assert_eq!(fs::read(&receipt_path).expect("原文件"), b"existing");
}
