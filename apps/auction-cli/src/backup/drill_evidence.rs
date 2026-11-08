//! 决策 31 月度恢复演练的只读证据：运维留存演练回执的目录中最新一次成功演练的完成时刻。
//!
//! 只认两类已经写出的成功回执，不执行恢复、不推断演练：
//! - `auction-postgres-restore.v1`：`backup restore` 只在恢复成功后写出，取 `restored_at`；
//! - `auction-migration-drill.v1`：W10.6 迁移演练同样恢复最新备份，只有 `conclusion = passed`
//!   且有 `finished_at` 才计入。
//!
//! 目录只读一层，跳过符号链接、非普通文件、超过 1 MiB 的文件与无法解析的 JSON；目录本身
//! 不可读时返回错误，由调用方把来源标记为不完整。

use std::{fs, path::Path};

use anyhow::Context;
use chrono::{DateTime, Utc};

/// 单份回执的读取上限；正常回执远小于该值。
const MAX_RECEIPT_BYTES: u64 = 1 << 20;
const RESTORE_FORMAT: &str = "auction-postgres-restore.v1";
const MIGRATION_DRILL_FORMAT: &str = "auction-migration-drill.v1";

/// 返回目录中最新一次成功恢复演练的完成时刻；没有任何成功回执时为 `None`。
pub(crate) fn latest_restore_drill(directory: &Path) -> anyhow::Result<Option<DateTime<Utc>>> {
    let entries = fs::read_dir(directory).context("读取恢复演练回执目录失败")?;
    let mut latest: Option<DateTime<Utc>> = None;
    for entry in entries {
        let entry = entry.context("枚举恢复演练回执失败")?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else { continue };
        if !file_name.ends_with(".json") {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).context("读取恢复演练回执属性失败")?;
        if !metadata.file_type().is_file() || metadata.len() > MAX_RECEIPT_BYTES {
            continue;
        }
        let bytes = fs::read(&path).context("读取恢复演练回执失败")?;
        let Ok(receipt) = serde_json::from_slice::<serde_json::Value>(&bytes) else { continue };
        if let Some(completed_at) = successful_drill_time(&receipt) {
            latest = Some(latest.map_or(completed_at, |current| current.max(completed_at)));
        }
    }
    Ok(latest)
}

/// 只从两类成功回执取完成时刻；其他格式、失败结论或缺少时间的回执都不是演练证据。
fn successful_drill_time(receipt: &serde_json::Value) -> Option<DateTime<Utc>> {
    let time = |field: &str| {
        receipt
            .get(field)?
            .as_str()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc))
    };
    match receipt.get("format")?.as_str()? {
        RESTORE_FORMAT => time("restored_at"),
        MIGRATION_DRILL_FORMAT
            if receipt.get("conclusion").and_then(serde_json::Value::as_str) == Some("passed") =>
        {
            time("finished_at")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicU32, Ordering},
    };

    use chrono::{DateTime, Utc};
    use serde_json::json;

    use super::latest_restore_drill;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "auction-restore-drill-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("创建测试根目录");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .expect("设置测试根目录权限");
            Self { root }
        }

        fn write(&self, name: &str, value: &serde_json::Value) {
            fs::write(self.root.join(name), serde_json::to_vec(value).expect("JSON"))
                .expect("写入回执");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value).expect("时间").with_timezone(&Utc)
    }

    #[test]
    fn picks_latest_successful_restore_or_passed_migration_drill() {
        let fixture = Fixture::new();
        fixture.write(
            "restore-old.json",
            &json!({"format": "auction-postgres-restore.v1", "restored_at": "2026-09-01T00:00:00Z"}),
        );
        fixture.write(
            "drill-passed.json",
            &json!({"format": "auction-migration-drill.v1", "conclusion": "passed",
                    "finished_at": "2026-10-15T12:00:00Z"}),
        );
        // 未通过的迁移演练、其他格式与非 JSON 文件都不是演练证据。
        fixture.write(
            "drill-incomplete.json",
            &json!({"format": "auction-migration-drill.v1", "conclusion": "incomplete",
                    "finished_at": "2026-11-01T00:00:00Z"}),
        );
        fixture.write(
            "backup-run.json",
            &json!({"format": "auction-backup-run.v1", "finished_at": "2026-11-02T00:00:00Z"}),
        );
        fs::write(fixture.root.join("notes.txt"), b"not a receipt").expect("写入杂项");
        fs::write(fixture.root.join("broken.json"), b"{").expect("写入损坏文件");
        assert_eq!(
            latest_restore_drill(&fixture.root).expect("目录可读"),
            Some(at("2026-10-15T12:00:00Z"))
        );
    }

    #[test]
    fn empty_directory_means_never_drilled_and_missing_directory_is_an_error() {
        let fixture = Fixture::new();
        assert_eq!(latest_restore_drill(&fixture.root).expect("目录可读"), None);
        assert!(latest_restore_drill(&fixture.root.join("missing")).is_err());
    }

    #[test]
    fn symlinked_receipts_are_ignored() {
        let fixture = Fixture::new();
        let outside = fixture.root.join("outside");
        fs::create_dir(&outside).expect("创建外部目录");
        fs::write(
            outside.join("restore.json"),
            serde_json::to_vec(&json!({"format": "auction-postgres-restore.v1",
                                       "restored_at": "2026-11-01T00:00:00Z"}))
            .expect("JSON"),
        )
        .expect("写入外部回执");
        let drills = fixture.root.join("drills");
        fs::create_dir(&drills).expect("创建演练目录");
        std::os::unix::fs::symlink(outside.join("restore.json"), drills.join("restore.json"))
            .expect("创建符号链接");
        assert_eq!(latest_restore_drill(&drills).expect("目录可读"), None);
    }
}
