use super::{EvidenceRetention, RecordEntry, classify_evidence, eviction_order, eviction_plan};
use auction_application::mail::MailDiagnosticError;
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_000)
}

fn record(name: &str, modified_seconds: u64, size: u64, write_unknown: bool) -> RecordEntry {
    RecordEntry {
        path: PathBuf::from(name),
        modified: UNIX_EPOCH + Duration::from_secs(modified_seconds),
        size,
        write_unknown,
    }
}

fn ordered(records: Vec<RecordEntry>) -> Vec<RecordEntry> {
    eviction_order(records).into()
}

fn names(records: &[RecordEntry]) -> Vec<String> {
    records.iter().map(|record| record.path.display().to_string()).collect()
}

fn plan(
    records: &[RecordEntry],
    incoming: u64,
    retention_seconds: u64,
    maximum_total_bytes: u64,
    hard_max_files: usize,
) -> Result<usize, MailDiagnosticError> {
    eviction_plan(
        records,
        now(),
        incoming,
        Duration::from_secs(retention_seconds),
        maximum_total_bytes,
        hard_max_files,
    )
}

#[test]
fn eviction_puts_write_unknown_evidence_after_ordinary_records() {
    let records = ordered(vec![
        record("unknown-oldest", 900, 10, true),
        record("busy-new", 990, 10, false),
        record("busy-old", 910, 10, false),
    ]);
    assert_eq!(names(&records), ["busy-old", "busy-new", "unknown-oldest"]);
}

#[test]
fn capacity_pressure_evicts_ordinary_evidence_before_write_unknown() {
    let first = ordered(vec![
        record("unknown-oldest", 900, 10, true),
        record("busy-new", 990, 10, false),
        record("busy-old", 910, 10, false),
    ]);
    // 文件数硬上限只剩 3 个槽位：本次写入必须先淘汰最早的普通证据。
    assert_eq!(plan(&first, 10, 86_400, 1_000, 3), Ok(1));
    assert_eq!(first[0].path, PathBuf::from("busy-old"));

    // 需要腾出两个槽位时才轮到第二个普通证据，写入未知证据仍然保留。
    let second = ordered(vec![
        record("unknown-oldest", 900, 10, true),
        record("busy-new", 990, 10, false),
        record("busy-old", 910, 10, false),
    ]);
    assert_eq!(plan(&second, 10, 86_400, 1_000, 2), Ok(2));
    assert!(second.iter().take(2).all(|record| !record.write_unknown));
}

#[test]
fn retention_removes_expired_ordinary_evidence_before_fresh_write_unknown() {
    let records = ordered(vec![
        record("unknown-fresh", 995, 10, true),
        record("busy-expired", 100, 10, false),
    ]);
    let evicted = plan(&records, 10, 10, 1_000, 1_024).expect("保留期淘汰");
    assert_eq!(evicted, 1);
    assert_eq!(records[0].path, PathBuf::from("busy-expired"));
    assert!(records[1].write_unknown);
}

#[test]
fn eviction_fails_closed_when_every_candidate_is_still_too_large() {
    let records = ordered(vec![record("only", 995, 10, true)]);
    assert_eq!(plan(&records, 2_000, 86_400, 1_000, 1_024), Err(MailDiagnosticError::Capacity));
}

#[test]
fn evidence_retention_boundaries_are_exact_and_type_aware() {
    let retention = Duration::from_secs(7 * 86_400);
    let hold = Duration::from_secs(90 * 86_400);
    // 差一秒不满 7 天：保留。
    assert_eq!(
        classify_evidence(retention - Duration::from_secs(1), retention, hold, false),
        EvidenceRetention::Retained
    );
    // 恰好满 7 天的普通证据：候选。
    assert_eq!(
        classify_evidence(retention, retention, hold, false),
        EvidenceRetention::OrdinaryExpired
    );
    // 满 7 天的写入未知证据：保全，不是候选。
    assert_eq!(
        classify_evidence(retention, retention, hold, true),
        EvidenceRetention::WriteUnknownHeld
    );
    // 未处置的写入未知证据即使超过 90 天也只标记跟进，仍不是候选。
    assert_eq!(
        classify_evidence(hold, retention, hold, true),
        EvidenceRetention::WriteUnknownOverHold
    );
}

#[test]
fn retention_snapshot_counts_ordinary_and_held_evidence() {
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    };
    use uuid::Uuid;

    struct Guard(std::path::PathBuf);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    let directory = std::env::temp_dir().join(format!("auction-retention-{}", Uuid::new_v4()));
    fs::DirBuilder::new().mode(0o700).create(&directory).expect("创建私有目录");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).expect("目录权限");
    let _guard = Guard(directory.clone());

    let write = |name: Uuid, failure_code: &str, modified: SystemTime| {
        let path = directory.join(format!("{name}.evidence"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .expect("创建证据文件");
        file.write_all(
            format!(
                "{{\"outcome\":{{\"failure_code\":\"{failure_code}\",\"decision_code\":null}}}}\nQUJD"
            )
            .as_bytes(),
        )
        .expect("写入证据文件");
        file.set_modified(modified).expect("设置修改时间");
    };

    let now = SystemTime::now();
    let old = now - Duration::from_secs(8 * 86_400);
    let fresh = now - Duration::from_secs(86_400);
    write(Uuid::new_v4(), "ordinary_failure", old);
    write(Uuid::new_v4(), "write_unknown", old);
    write(Uuid::new_v4(), "ordinary_failure", fresh);

    let snapshot = super::retention_snapshot(
        &directory,
        Duration::from_secs(7 * 86_400),
        Duration::from_secs(90 * 86_400),
        now,
    )
    .expect("统计证据保留快照");
    assert!(snapshot.directory_present);
    assert_eq!(snapshot.total_files, 3);
    assert_eq!(snapshot.ordinary_expired_files, 1);
    assert_eq!(snapshot.write_unknown_held_files, 1);
    assert_eq!(snapshot.write_unknown_over_hold_window_files, 0);
    assert_eq!(snapshot.retained_files, 1);
    assert_eq!(snapshot.unexpected_entries, 0);
    assert_eq!(snapshot.temporary_files, 0);
    assert!(snapshot.ordinary_expired_bytes > 0);
    assert!(snapshot.write_unknown_held_bytes > 0);

    let missing = directory.join("absent");
    let absent = super::retention_snapshot(
        &missing,
        Duration::from_secs(7 * 86_400),
        Duration::from_secs(90 * 86_400),
        now,
    )
    .expect("缺失目录按空快照");
    assert!(!absent.directory_present);
    assert_eq!(absent.total_files, 0);
}

#[test]
fn purge_deletes_only_approved_ordinary_evidence_and_is_idempotent() {
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    };
    use uuid::Uuid;

    struct Guard(std::path::PathBuf);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    let directory = std::env::temp_dir().join(format!("auction-purge-{}", Uuid::new_v4()));
    fs::DirBuilder::new().mode(0o700).create(&directory).expect("创建私有目录");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).expect("目录权限");
    let _guard = Guard(directory.clone());

    let write = |failure_code: &str, modified: SystemTime, padding: &str| {
        let path = directory.join(format!("{}.evidence", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .expect("创建证据文件");
        file.write_all(
            format!(
                "{{\"outcome\":{{\"failure_code\":\"{failure_code}\",\"decision_code\":null}}}}\nQUJD{padding}"
            )
            .as_bytes(),
        )
        .expect("写入证据文件");
        file.set_modified(modified).expect("设置修改时间");
        path
    };

    let now = SystemTime::now();
    let old = now - Duration::from_secs(10 * 86_400);
    let older = write("ordinary_failure", old - Duration::from_secs(3_600), "AA");
    let newer = write("ordinary_failure", old, "BBBB");
    let held = write("write_unknown", old, "");
    let over_hold = write("write_unknown", now - Duration::from_secs(100 * 86_400), "");
    let retained = write("ordinary_failure", now - Duration::from_secs(86_400), "");
    let older_len = fs::metadata(&older).expect("旧文件元数据").len();
    let newer_len = fs::metadata(&newer).expect("新文件元数据").len();

    let first = super::purge_ordinary_expired(
        &directory,
        Duration::from_secs(7 * 86_400),
        Duration::from_secs(90 * 86_400),
        now,
        1,
    )
    .expect("清理候选");
    assert!(first.directory_present);
    assert_eq!(first.examined_files, 5);
    assert_eq!(first.deleted_files, 1);
    assert_eq!(first.deleted_bytes, older_len);
    assert!(first.limit_reached, "候选多于批准上限");
    assert!(first.needs_follow_up, "超过保全窗的写入未知证据需人工跟进");
    assert!(first.failure_code.is_none());
    assert!(!older.exists(), "最早的普通证据被删除");
    assert!(newer.exists() && held.exists() && over_hold.exists() && retained.exists());

    let second = super::purge_ordinary_expired(
        &directory,
        Duration::from_secs(7 * 86_400),
        Duration::from_secs(90 * 86_400),
        now,
        10,
    )
    .expect("再次清理候选");
    assert_eq!(second.deleted_files, 1);
    assert_eq!(second.deleted_bytes, newer_len);
    assert!(!second.limit_reached);
    assert!(!newer.exists());
    assert!(held.exists(), "写入未知证据永不在本次清理范围");
    assert!(over_hold.exists(), "超过保全窗也只人工跟进，不自动删除");
    assert!(retained.exists());

    let third = super::purge_ordinary_expired(
        &directory,
        Duration::from_secs(7 * 86_400),
        Duration::from_secs(90 * 86_400),
        now,
        10,
    )
    .expect("无候选重跑");
    assert_eq!(third.deleted_files, 0);
    assert_eq!(third.deleted_bytes, 0);
    assert_eq!(third.examined_files, 3);
    assert!(third.failure_code.is_none());
    assert!(held.exists() && over_hold.exists() && retained.exists());
}
