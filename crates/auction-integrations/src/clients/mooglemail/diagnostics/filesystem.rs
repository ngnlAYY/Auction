//! 证据文件格式与平台边界；按最终落盘字节计算容量。

use auction_application::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use auction_application::MailDiagnosticError;
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
#[path = "filesystem/unix.rs"]
mod platform;
#[cfg(not(unix))]
#[path = "filesystem/unsupported.rs"]
mod platform;

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
        || !matches!(config.key_source.as_str(), "credential" | "equipment")
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
    validate_config(config)?;
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
    use super::{RecordEntry, eviction_order, eviction_plan};
    use auction_application::MailDiagnosticError;
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
}
