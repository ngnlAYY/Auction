//! 证据文件格式与平台边界；按最终落盘字节计算容量。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::safe_diagnostics;

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
mod tests;
