//! 主机侧告警报告使用的只读证据：备份目录文件系统水位与最近一次终态回执。
//!
//! 报告命令只读取已经发布的证据：`statvfs` 的非特权可用块、`<state-dir>/receipts/` 下的
//! 终态 JSON。它不创建目录、不取得备份锁、不写回执，也不把 `running.json` 或无法解析的
//! 文件当作终态。

use std::{
    fs::{self, File},
    io::ErrorKind,
    path::Path,
};

use anyhow::Context;
use chrono::{DateTime, Utc};

use super::filesystem::MAX_RECORD_BYTES;
use super::model::{FORMAT, Outcome, RunReceipt};

/// 文件系统水位；阈值来自 `[backup]` 配置，由调用方读取。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiskUsage {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

/// 终态回执的结果闭集；`running` 不是终态证据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalOutcome {
    Completed,
    Failed,
    Interrupted,
}

/// 最近一次终态运行；只保留报告需要的结束时间与结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalRun {
    pub outcome: TerminalOutcome,
    pub finished_at: DateTime<Utc>,
}

/// 读取备份输出目录所在文件系统的总量与非特权可用字节。
pub(crate) fn disk_usage(directory: &Path) -> anyhow::Result<DiskUsage> {
    let file = File::open(directory).context("打开备份目录失败")?;
    let metadata = file.metadata().context("读取备份目录属性失败")?;
    if !metadata.is_dir() {
        anyhow::bail!("备份输出路径不是目录");
    }
    let usage = super::disk::usage(&file)?;
    Ok(DiskUsage { total_bytes: usage.total_bytes, available_bytes: usage.available_bytes })
}

/// 读取 `<state-dir>/receipts/` 中开始时间最晚的终态回执。
///
/// 目录不存在或没有任何终态回执时返回 `None`：报告层据此把来源标记为证据不完整，
/// 而不是假称“备份正常”。无法解析或身份不符的文件单独跳过，不阻止其他证据。
pub(crate) fn latest_terminal_run(state_dir: &Path) -> anyhow::Result<Option<TerminalRun>> {
    let receipts = state_dir.join("receipts");
    let entries = match fs::read_dir(&receipts) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("读取备份回执目录失败"),
    };
    let mut latest: Option<(DateTime<Utc>, TerminalOutcome, DateTime<Utc>)> = None;
    for entry in entries {
        let entry = entry.context("枚举备份回执失败")?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else { continue };
        if !file_name.ends_with(".json") {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).context("读取备份回执属性失败")?;
        if !metadata.file_type().is_file() || metadata.len() > MAX_RECORD_BYTES {
            continue;
        }
        let bytes = fs::read(&path).context("读取备份回执失败")?;
        let Ok(receipt) = serde_json::from_slice::<RunReceipt>(&bytes) else {
            continue;
        };
        if receipt.format != FORMAT {
            continue;
        }
        let Some(finished_at) = receipt.finished_at else { continue };
        let outcome = match receipt.outcome {
            Outcome::Completed => TerminalOutcome::Completed,
            Outcome::Failed => TerminalOutcome::Failed,
            Outcome::Interrupted => TerminalOutcome::Interrupted,
            Outcome::Running => continue,
        };
        let replace = latest.is_none_or(|(started_at, _, _)| receipt.started_at > started_at);
        if replace {
            latest = Some((receipt.started_at, outcome, finished_at));
        }
    }
    Ok(latest.map(|(_, outcome, finished_at)| TerminalRun { outcome, finished_at }))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicU32, Ordering},
    };

    use serde_json::json;

    use super::{TerminalOutcome, disk_usage, latest_terminal_run};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// 只读回执目录的最小夹具；测试结束删除整个临时根目录。
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "auction-backup-evidence-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("创建测试根目录");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .expect("设置测试根目录权限");
            fs::create_dir(root.join("receipts")).expect("创建回执目录");
            Self { root }
        }

        fn write_receipt(&self, name: &str, value: &serde_json::Value) {
            fs::write(self.root.join("receipts").join(name), serde_json::to_vec(value).unwrap())
                .expect("写入回执");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn receipt(started_at: &str, finished_at: &str, outcome: &str) -> serde_json::Value {
        json!({
            "format": "auction-backup-run.v1",
            "run_id": "6f9d6b5a-4d3f-4e68-9c39-3a1f2f3f4f50",
            "started_at": started_at,
            "finished_at": finished_at,
            "elapsed_millis": 1_000,
            "stage_timings": [],
            "outcome": outcome,
            "stage": "receipt",
            "database_name_sha256": "0".repeat(64),
            "output_dir": "/var/backups/auction/scheduled",
            "policy": null,
            "archive": null,
            "disk_before": null,
            "disk_after": null,
            "cleanup": {"removed": [], "removed_count": 0, "skipped": [], "skipped_count": 0},
            "recovered_interrupted": 0,
            "failure": null,
            "heartbeat_configured": false
        })
    }

    #[test]
    fn latest_terminal_run_picks_the_newest_terminal_receipt() {
        let fixture = Fixture::new();
        fixture.write_receipt(
            "old.json",
            &receipt("2026-11-01T03:00:00Z", "2026-11-01T03:10:00Z", "completed"),
        );
        fixture.write_receipt(
            "new.json",
            &receipt("2026-11-02T03:00:00Z", "2026-11-02T03:20:00Z", "failed"),
        );
        // 运行中记录与无法解析的文件都不是终态证据。
        fixture.write_receipt(
            "running.json",
            &receipt("2026-11-03T03:00:00Z", "2026-11-03T03:00:00Z", "running"),
        );
        fs::write(fixture.root.join("receipts").join("broken.json"), b"not json")
            .expect("写入损坏回执");

        let run = latest_terminal_run(&fixture.root).expect("读取回执").expect("必须找到终态回执");
        assert_eq!(run.outcome, TerminalOutcome::Failed);
        assert_eq!(run.finished_at.to_rfc3339(), "2026-11-02T03:20:00+00:00");
    }

    #[test]
    fn latest_terminal_run_is_none_without_receipt_evidence() {
        let fixture = Fixture::new();
        assert_eq!(latest_terminal_run(&fixture.root).expect("读取回执"), None);
        // 目录不存在同样只是“没有证据”，不是读取失败。
        let missing = fixture.root.join("missing");
        assert_eq!(latest_terminal_run(&missing).expect("读取回执"), None);
    }

    #[test]
    fn disk_usage_reports_the_backup_filesystem() {
        let fixture = Fixture::new();
        let usage = disk_usage(&fixture.root).expect("读取文件系统水位");
        assert!(usage.total_bytes > 0);
        assert!(usage.available_bytes <= usage.total_bytes);
    }

    #[test]
    fn disk_usage_rejects_a_missing_directory() {
        let fixture = Fixture::new();
        assert!(disk_usage(&fixture.root.join("missing")).is_err());
    }
}
