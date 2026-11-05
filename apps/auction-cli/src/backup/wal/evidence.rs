//! 主机侧告警报告使用的只读 WAL 暂存证据。
//!
//! 上传器在远端校验通过后删除本地密文与 manifest，因此暂存目录里“密文 + manifest 齐全”
//! 的 WAL 段就是尚未上传的积压。归档进程刚发布密文、尚未发布 manifest 的项属于进行中，
//! 不参与积压判定；格式错误的项单独计数，不阻断其他证据。

use std::path::Path;

use super::model::{require_staging_dir, staged};

/// 尚未上传的 WAL 段水位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StagedWalFacts {
    /// 已发布 manifest、等待上传的段数量。
    pub staged_segments: i64,
    /// 最老暂存段自归档完成以来的秒数；没有暂存段时为 0。
    pub oldest_staged_age_seconds: i64,
    /// 无法解析的暂存项数量；只用于报告，不参与阈值。
    pub invalid_entries: i64,
}

/// 读取暂存目录中等待上传的 WAL 段。
///
/// `now_unix_seconds` 由调用方用同一时钟取得；manifest 的 `archived_unix_seconds` 超前
/// （时钟回拨）时按 0 秒处理，不产生负年龄。
pub(crate) fn staged_wal_facts(
    directory: &Path,
    now_unix_seconds: u64,
) -> anyhow::Result<StagedWalFacts> {
    require_staging_dir(directory)?;
    let scan = staged(directory)?;
    let mut oldest = 0_u64;
    for item in &scan.ready {
        let archived = item.record.archived_unix_seconds;
        let age = now_unix_seconds.saturating_sub(archived);
        oldest = oldest.max(age);
    }
    Ok(StagedWalFacts {
        staged_segments: i64::try_from(scan.ready.len()).unwrap_or(i64::MAX),
        oldest_staged_age_seconds: i64::try_from(oldest).unwrap_or(i64::MAX),
        invalid_entries: i64::try_from(scan.invalid.len()).unwrap_or(i64::MAX),
    })
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

    use super::staged_wal_facts;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "auction-wal-evidence-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("创建测试根目录");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .expect("设置测试根目录权限");
            Self { root }
        }

        fn segment(&self, name: &str, cipher: &[u8], archived_unix_seconds: u64) {
            let cipher_path = self.root.join(format!("{name}.zst.age"));
            fs::write(&cipher_path, cipher).expect("写入密文");
            let manifest = json!({
                "format": "auction-wal-archive.v1",
                "segment_name": name,
                "source_bytes": 16,
                "source_mtime_ns": 1,
                "cipher_bytes": cipher.len(),
                "archived_unix_seconds": archived_unix_seconds,
            });
            fs::write(
                self.root.join(format!("{name}.zst.age.manifest.json")),
                serde_json::to_vec(&manifest).expect("序列化 manifest"),
            )
            .expect("写入 manifest");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn staged_wal_facts_count_ready_segments_and_oldest_age() {
        let fixture = Fixture::new();
        fixture.segment("000000010000000000000001", b"cipher-1", 1_000);
        fixture.segment("000000010000000000000002", b"cipher-2", 1_900);
        // 只有 manifest、缺密文：归档尚未确认，不参与积压判定。
        fs::write(fixture.root.join("000000010000000000000003.zst.age.manifest.json"), b"{}")
            .expect("写入残缺 manifest");

        let facts = staged_wal_facts(&fixture.root, 2_000).expect("读取暂存事实");
        assert_eq!(facts.staged_segments, 2);
        assert_eq!(facts.oldest_staged_age_seconds, 1_000);
        assert_eq!(facts.invalid_entries, 0);
    }

    #[test]
    fn staged_wal_facts_treat_clock_skew_as_zero_age() {
        let fixture = Fixture::new();
        fixture.segment("000000010000000000000004", b"cipher", 5_000);
        let facts = staged_wal_facts(&fixture.root, 1_000).expect("读取暂存事实");
        assert_eq!(facts.staged_segments, 1);
        assert_eq!(facts.oldest_staged_age_seconds, 0);
    }

    #[test]
    fn empty_staging_is_quiet() {
        let fixture = Fixture::new();
        let facts = staged_wal_facts(&fixture.root, 2_000).expect("读取暂存事实");
        assert_eq!(facts.staged_segments, 0);
        assert_eq!(facts.oldest_staged_age_seconds, 0);
        assert_eq!(facts.invalid_entries, 0);
    }
}
