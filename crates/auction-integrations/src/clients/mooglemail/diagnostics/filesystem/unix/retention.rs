//! 只读保留快照与普通过期证据清理；写入未知证据一律保留。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::safe_diagnostics;

use std::{
    fs::{self, File},
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use auction_application::mail::MailDiagnosticError;

use super::{
    lock::locked_directory,
    path::normalized_target,
    private::private_file,
    records::{managed_name, write_unknown_evidence},
};

use super::super::{
    EvidenceRetention, MailDiagnosticPurgeOutcome, MailDiagnosticRetentionSnapshot,
    classify_evidence,
};

pub(in super::super) fn retention_snapshot(
    directory: &Path,
    retention: Duration,
    hold: Duration,
    now: SystemTime,
) -> Result<MailDiagnosticRetentionSnapshot, MailDiagnosticError> {
    let path = normalized_target(directory)?;
    let directory = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return if error.kind() == io::ErrorKind::NotFound {
                Ok(MailDiagnosticRetentionSnapshot::default())
            } else {
                Err(MailDiagnosticError::Io)
            };
        }
    };
    if directory.file_type().is_symlink() || !directory.is_dir() {
        return Err(MailDiagnosticError::Io);
    }
    let owner = directory.uid();
    let mut snapshot =
        MailDiagnosticRetentionSnapshot { directory_present: true, ..Default::default() };
    for entry in fs::read_dir(&path).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "list_retention_snapshot",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })? {
        let entry = entry.map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "read_retention_entry",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            snapshot.unexpected_entries = snapshot.unexpected_entries.saturating_add(1);
            continue;
        };
        if managed_name(name, false).is_some() {
            let metadata = private_file(&entry.path(), owner, false)?;
            let modified = metadata.modified().map_err(|diagnostic_cause| {
                safe_diagnostics::error(
                    "mooglemail_diagnostics",
                    "read_retention_timestamp",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                MailDiagnosticError::Io
            })?;
            let age = now.duration_since(modified).unwrap_or(Duration::ZERO);
            snapshot.total_files = snapshot.total_files.saturating_add(1);
            let write_unknown =
                if age >= retention { write_unknown_evidence(&entry.path(), owner) } else { false };
            match classify_evidence(age, retention, hold, write_unknown) {
                EvidenceRetention::Retained => {
                    snapshot.retained_files = snapshot.retained_files.saturating_add(1);
                }
                EvidenceRetention::OrdinaryExpired => {
                    snapshot.ordinary_expired_files =
                        snapshot.ordinary_expired_files.saturating_add(1);
                    snapshot.ordinary_expired_bytes =
                        snapshot.ordinary_expired_bytes.saturating_add(metadata.len());
                }
                EvidenceRetention::WriteUnknownHeld => {
                    snapshot.write_unknown_held_files =
                        snapshot.write_unknown_held_files.saturating_add(1);
                    snapshot.write_unknown_held_bytes =
                        snapshot.write_unknown_held_bytes.saturating_add(metadata.len());
                }
                EvidenceRetention::WriteUnknownOverHold => {
                    snapshot.write_unknown_held_files =
                        snapshot.write_unknown_held_files.saturating_add(1);
                    snapshot.write_unknown_held_bytes =
                        snapshot.write_unknown_held_bytes.saturating_add(metadata.len());
                    snapshot.write_unknown_over_hold_window_files =
                        snapshot.write_unknown_over_hold_window_files.saturating_add(1);
                }
            }
        } else if managed_name(name, true).is_some() {
            snapshot.temporary_files = snapshot.temporary_files.saturating_add(1);
        } else {
            snapshot.unexpected_entries = snapshot.unexpected_entries.saturating_add(1);
        }
    }
    Ok(snapshot)
}

pub(in super::super) fn purge_ordinary_expired(
    directory: &Path,
    retention: Duration,
    hold: Duration,
    now: SystemTime,
    max_files: u64,
) -> Result<MailDiagnosticPurgeOutcome, MailDiagnosticError> {
    let target = normalized_target(directory)?;
    let locked = match locked_directory(&target, false) {
        Ok(locked) => locked,
        Err(MailDiagnosticError::NotFound) => {
            return Ok(MailDiagnosticPurgeOutcome::default());
        }
        Err(error) => return Err(error),
    };
    let owner = locked.owner;
    let mut outcome = MailDiagnosticPurgeOutcome {
        directory_present: true,
        ..MailDiagnosticPurgeOutcome::default()
    };
    let mut candidates: Vec<(PathBuf, SystemTime, u64)> = Vec::new();
    for entry in fs::read_dir(&locked.path).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "list_purge_entries",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })? {
        let entry = entry.map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "read_purge_entry",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            outcome.unexpected_entries = outcome.unexpected_entries.saturating_add(1);
            continue;
        };
        if name == ".lock" {
            // 本模块自己的目录锁；不是证据文件。
            continue;
        }
        if managed_name(name, false).is_some() {
            let metadata = private_file(&entry.path(), owner, false)?;
            let modified = metadata.modified().map_err(|diagnostic_cause| {
                safe_diagnostics::error(
                    "mooglemail_diagnostics",
                    "read_purge_timestamp",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                MailDiagnosticError::Io
            })?;
            let age = now.duration_since(modified).unwrap_or(Duration::ZERO);
            outcome.examined_files = outcome.examined_files.saturating_add(1);
            let write_unknown =
                if age >= retention { write_unknown_evidence(&entry.path(), owner) } else { false };
            match classify_evidence(age, retention, hold, write_unknown) {
                EvidenceRetention::Retained => {
                    outcome.retained_files = outcome.retained_files.saturating_add(1);
                }
                EvidenceRetention::OrdinaryExpired => {
                    candidates.push((entry.path(), modified, metadata.len()));
                }
                EvidenceRetention::WriteUnknownHeld => {
                    outcome.write_unknown_held_files =
                        outcome.write_unknown_held_files.saturating_add(1);
                }
                EvidenceRetention::WriteUnknownOverHold => {
                    outcome.write_unknown_held_files =
                        outcome.write_unknown_held_files.saturating_add(1);
                    outcome.write_unknown_over_hold_window_files =
                        outcome.write_unknown_over_hold_window_files.saturating_add(1);
                    outcome.needs_follow_up = true;
                }
            }
        } else if managed_name(name, true).is_some() {
            // 临时文件可能属于在途写入；不计数也不删除。
        } else {
            outcome.unexpected_entries = outcome.unexpected_entries.saturating_add(1);
        }
    }
    if outcome.unexpected_entries > 0 {
        outcome.needs_follow_up = true;
    }
    if candidates.len() as u64 > max_files {
        outcome.limit_reached = true;
    }
    // 先按修改时间从旧到新排序，再按批准上限截断：超限时必须删除最旧的候选。
    candidates.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
    candidates.truncate(usize::try_from(max_files).unwrap_or(usize::MAX));
    for (path, _, _) in &candidates {
        let Ok(before) = private_file(path, owner, false) else {
            outcome.failure_code = Some("evidence_file_changed");
            break;
        };
        if fs::remove_file(path).is_err() {
            outcome.failure_code = Some("evidence_delete_failed");
            break;
        }
        outcome.deleted_files = outcome.deleted_files.saturating_add(1);
        outcome.deleted_bytes = outcome.deleted_bytes.saturating_add(before.len());
    }
    if outcome.failure_code.is_none()
        && File::open(&locked.path).and_then(|directory| directory.sync_all()).is_err()
    {
        outcome.failure_code = Some("evidence_directory_sync_failed");
    }
    Ok(outcome)
}
