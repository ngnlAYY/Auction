//! 证据文件格式与平台边界；按最终落盘字节计算容量。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use auction_application::mail::MailDiagnosticError;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::{
    collections::VecDeque,
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime},
};
use uuid::Uuid;

use super::{
    MoogleMailDiagnosticArchiveConfig,
    payload::{self, PublicHeader},
};

#[cfg(unix)]
mod unix;
#[cfg(not(unix))]
mod unsupported;

#[cfg(unix)]
use unix as platform;
#[cfg(not(unix))]
use unsupported as platform;

const MAX_HEADER_BYTES: usize = 8 * 1024;
const MAX_CIPHERTEXT_BYTES: usize = payload::MAX_PAYLOAD + 12 + 16;
const MAX_FILE_BYTES: usize = MAX_CIPHERTEXT_BYTES.div_ceil(3) * 4 + MAX_HEADER_BYTES + 1;

/// 已归档证据的淘汰元数据；只描述文件身份、时间和占用，不携带证据正文。
pub(super) struct RecordEntry {
    pub(super) path: PathBuf,
    pub(super) modified: SystemTime,
    pub(super) size: u64,
    /// 结果未知（写入未知）的证据必须晚于其他证据被淘汰。
    pub(super) write_unknown: bool,
}

/// 加密诊断证据的只读保留快照；只统计文件数与字节数，不读取或导出正文。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MailDiagnosticRetentionSnapshot {
    pub directory_present: bool,
    pub total_files: u64,
    pub ordinary_expired_files: u64,
    pub ordinary_expired_bytes: u64,
    pub write_unknown_held_files: u64,
    pub write_unknown_held_bytes: u64,
    /// 已超过处置后 90 天保全窗、需人工跟进处置状态的写入未知证据数；不是删除候选。
    pub write_unknown_over_hold_window_files: u64,
    pub retained_files: u64,
    pub temporary_files: u64,
    pub unexpected_entries: u64,
}

/// 单个证据文件按修改时间与是否写入未知分类；边界为 `age >= 期限`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EvidenceRetention {
    Retained,
    OrdinaryExpired,
    WriteUnknownHeld,
    WriteUnknownOverHold,
}

pub(super) fn classify_evidence(
    age: Duration,
    retention: Duration,
    hold: Duration,
    write_unknown: bool,
) -> EvidenceRetention {
    if age < retention {
        return EvidenceRetention::Retained;
    }
    if !write_unknown {
        return EvidenceRetention::OrdinaryExpired;
    }
    if age >= hold {
        EvidenceRetention::WriteUnknownOverHold
    } else {
        EvidenceRetention::WriteUnknownHeld
    }
}

/// 单次清理最多删除的证据文件数；与归档层的硬上限一致。
const MAX_PURGE_FILES: u64 = 1024;

/// 已批准清理的文件族结果；只统计按同一分类判定为普通过期并实际 unlink 的文件。
/// `failure_code` 非空表示清理中途停止，之前的删除已生效且计数仍然精确。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MailDiagnosticPurgeOutcome {
    pub directory_present: bool,
    pub examined_files: u64,
    pub deleted_files: u64,
    pub deleted_bytes: u64,
    pub retained_files: u64,
    pub write_unknown_held_files: u64,
    pub write_unknown_over_hold_window_files: u64,
    pub unexpected_entries: u64,
    /// 候选数超过本次批准上限；未被删除的候选留给下一次重新演练与批准。
    pub limit_reached: bool,
    /// 目录缺失或超过保全窗的写入未知证据需要人工跟进；这些文件永不清理。
    pub needs_follow_up: bool,
    pub failure_code: Option<&'static str>,
}

fn validate_retention_parameters(
    directory: &Path,
    retention: Duration,
    hold: Duration,
) -> Result<(), MailDiagnosticError> {
    if directory.as_os_str().is_empty()
        || directory.file_name().is_none()
        || directory.to_string_lossy().chars().any(char::is_control)
        || matches!(
            directory.components().next_back(),
            Some(Component::CurDir | Component::ParentDir | Component::RootDir)
        )
        || retention < Duration::from_secs(86_400)
        || retention > Duration::from_secs(365 * 86_400)
        || hold < retention
        || hold > Duration::from_secs(365 * 86_400)
    {
        return Err(MailDiagnosticError::InvalidContext);
    }
    Ok(())
}

/// 只读统计诊断目录；目录缺失视为空快照，其他权限或格式异常失败关闭。
/// 不需要父密钥：本函数只读取文件元数据与公开头，不导出正文。
pub fn retention_snapshot(
    directory: &Path,
    retention: Duration,
    hold: Duration,
    now: SystemTime,
) -> Result<MailDiagnosticRetentionSnapshot, MailDiagnosticError> {
    validate_retention_parameters(directory, retention, hold)?;
    platform::retention_snapshot(directory, retention, hold, now)
}

/// 按政策 v2 删除普通过期证据文件，最多 `max_files` 个。写入结果未知的证据一律保留，
/// 包括超过 90 天保全窗的记录；临时文件与未识别目录项不删除。参数或目录身份异常时
/// 在删除任何文件前失败关闭；删除中途失败仍在返回值中精确报告已完成的删除。
pub fn purge_ordinary_expired(
    directory: &Path,
    retention: Duration,
    hold: Duration,
    now: SystemTime,
    max_files: u64,
) -> Result<MailDiagnosticPurgeOutcome, MailDiagnosticError> {
    validate_retention_parameters(directory, retention, hold)?;
    if max_files > MAX_PURGE_FILES {
        return Err(MailDiagnosticError::InvalidContext);
    }
    platform::purge_ordinary_expired(directory, retention, hold, now, max_files)
}

/// 结果未知证据的公开失败码；与 `MailDiagnosticOutcome::failure_code` 的闭集一致。
/// 只有已证明“远端写入结果未知”的证据享受保全优先级，其他分类按普通证据淘汰。
pub(super) const WRITE_UNKNOWN_FAILURE_CODE: &str = "write_unknown";

/// 公开头中判定淘汰优先级所需的最小投影；忽略正文与其他公开字段。
#[derive(serde::Deserialize)]
pub(super) struct EvictionHeader {
    pub(super) outcome: payload::OutcomeWire,
}

/// 淘汰顺序：普通证据按修改时间由旧到新在前，写入未知证据一律排在其后。
/// 保留期与容量压力下都先删最早的忙碌页等普通证据，只有没有更差候选时才动写入未知证据。
pub(super) fn eviction_order(mut records: Vec<RecordEntry>) -> VecDeque<RecordEntry> {
    records.sort_by(|left, right| {
        left.write_unknown
            .cmp(&right.write_unknown)
            .then_with(|| left.modified.cmp(&right.modified))
    });
    records.into()
}

/// 计算本次写入必须从 `eviction_order` 头部删除的记录数量。
/// 保留期、文件数硬上限或总容量任一超限都继续淘汰；全部候选仍放不下时返回容量错误。
/// 该函数只决定删除顺序与数量，不执行任何 IO。
pub(super) fn eviction_plan(
    records: &[RecordEntry],
    now: SystemTime,
    incoming: u64,
    retention: Duration,
    maximum_total_bytes: u64,
    hard_max_files: usize,
) -> Result<usize, MailDiagnosticError> {
    let mut total = records
        .iter()
        .try_fold(0u64, |sum, record| sum.checked_add(record.size))
        .ok_or(MailDiagnosticError::Capacity)?;
    let mut evicted = 0;
    while let Some(record) = records.get(evicted) {
        let expired = now.duration_since(record.modified).is_ok_and(|age| age >= retention);
        let too_many = records.len() - evicted >= hard_max_files;
        let too_large = total.checked_add(incoming).is_none_or(|size| size > maximum_total_bytes);
        if !expired && !too_many && !too_large {
            break;
        }
        total -= record.size;
        evicted += 1;
    }
    if total.checked_add(incoming).is_none_or(|size| size > maximum_total_bytes) {
        return Err(MailDiagnosticError::Capacity);
    }
    Ok(evicted)
}

pub(super) fn validate_config(
    config: &MoogleMailDiagnosticArchiveConfig,
) -> Result<(), MailDiagnosticError> {
    validate_layout(config)?;
    if !matches!(config.key_source.as_str(), "credential" | "equipment" | "mail_diagnostics") {
        return Err(MailDiagnosticError::InvalidContext);
    }
    Ok(())
}

/// 目录与保留预算校验；离线导出在读取头部前还不知道写入域，因此不校验 `key_source`。
fn validate_layout(config: &MoogleMailDiagnosticArchiveConfig) -> Result<(), MailDiagnosticError> {
    if config.directory.as_os_str().is_empty()
        || config.directory.file_name().is_none()
        || config.directory.to_string_lossy().chars().any(char::is_control)
        || matches!(
            config.directory.components().next_back(),
            Some(Component::CurDir | Component::ParentDir | Component::RootDir)
        )
        || config.retention < Duration::from_secs(86_400)
        || config.retention > Duration::from_secs(365 * 86_400)
        || !(16 * 1024 * 1024..=1024 * 1024 * 1024).contains(&config.maximum_total_bytes)
    {
        return Err(MailDiagnosticError::InvalidContext);
    }
    Ok(())
}

pub(super) fn persist(
    config: &MoogleMailDiagnosticArchiveConfig,
    header: &PublicHeader,
    ciphertext: &[u8],
) -> Result<(), MailDiagnosticError> {
    validate_config(config)?;
    payload::validate_header(header, header.evidence_id)?;
    let mut encoded = serde_json::to_string(header).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "encode_record_header",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Serialization
    })?;
    if encoded.len() > MAX_HEADER_BYTES || ciphertext.len() > MAX_CIPHERTEXT_BYTES {
        return Err(MailDiagnosticError::PayloadTooLarge);
    }
    encoded.push('\n');
    // 直接追加到最终文件缓冲，避免先分配并复制整份 Base64 密文；文件字节格式保持不变。
    BASE64.encode_string(ciphertext, &mut encoded);
    if encoded.len() > MAX_FILE_BYTES || encoded.len() as u64 > config.maximum_total_bytes {
        return Err(MailDiagnosticError::Capacity);
    }
    platform::persist(config, header.evidence_id, encoded.as_bytes())
}

pub(super) fn read(
    config: &MoogleMailDiagnosticArchiveConfig,
    id: Uuid,
) -> Result<(PublicHeader, Vec<u8>), MailDiagnosticError> {
    validate_layout(config)?;
    if id.is_nil() {
        return Err(MailDiagnosticError::InvalidContext);
    }
    let encoded = platform::read(config, id)?;
    let split = encoded
        .iter()
        .position(|byte| *byte == b'\n')
        .filter(|split| *split <= MAX_HEADER_BYTES)
        .ok_or(MailDiagnosticError::Authentication)?;
    let header: PublicHeader =
        serde_json::from_slice(&encoded[..split]).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "decode_record_header",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Authentication
        })?;
    payload::validate_header(&header, id)?;
    let ciphertext = BASE64.decode(&encoded[split + 1..]).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "decode_record_ciphertext",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Authentication
    })?;
    if ciphertext.len() > MAX_CIPHERTEXT_BYTES {
        return Err(MailDiagnosticError::PayloadTooLarge);
    }
    Ok((header, ciphertext))
}

pub(super) fn write_private_export(output: &Path, bytes: &[u8]) -> Result<(), MailDiagnosticError> {
    platform::write_private_export(output, bytes)
}

#[cfg(test)]
mod tests {
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
}
