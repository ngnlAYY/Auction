//! 证据目录清单：严格命名识别、临时文件清理和写入未知证据分类。

use std::{fs, io::Read, os::unix::fs::MetadataExt, path::Path};

use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::mail::MailDiagnosticError;
use uuid::Uuid;

use super::super::{
    EvictionHeader, MAX_FILE_BYTES, MAX_HEADER_BYTES, RecordEntry, WRITE_UNKNOWN_FAILURE_CODE,
};
use super::{
    lock::LockedDirectory,
    path::same_file,
    private::{open_private, private_file},
};
use crate::safe_diagnostics;

/// 目录内受管文件数（证据 + 临时）的硬上限，超出视为容量异常。
pub(super) const HARD_MAX_FILES: usize = 1024;

pub(super) fn managed_name(name: &str, temporary: bool) -> Option<Uuid> {
    let raw = if temporary {
        name.strip_prefix('.')?.strip_suffix(".tmp")?
    } else {
        name.strip_suffix(".evidence")?
    };
    let id = Uuid::parse_str(raw).ok()?;
    (!id.is_nil() && id.to_string() == raw).then_some(id)
}

pub(super) fn remove_file(path: &Path) -> Result<(), MailDiagnosticError> {
    fs::remove_file(path).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "remove_record",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })
}

/// 只清理严格命名的本模块临时文件；先收集再清理，未知文件使整次操作失败。
pub(super) fn collect_records(
    directory: &LockedDirectory,
) -> Result<Vec<RecordEntry>, MailDiagnosticError> {
    let mut temporary = Vec::new();
    let mut records = Vec::new();
    for entry in fs::read_dir(&directory.path).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "list_records",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })? {
        let entry = entry.map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "read_record_entry",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?;
        let name = entry.file_name();
        let name = name.to_str().ok_or(MailDiagnosticError::Io)?;
        if name == ".lock" {
            private_file(&entry.path(), directory.owner, false)?;
            continue;
        }
        if let Some(id) = managed_name(name, true) {
            temporary.push((entry.path(), id));
        } else if managed_name(name, false).is_some() {
            records.push(entry.path());
        } else {
            return Err(MailDiagnosticError::Io);
        }
        if temporary.len() + records.len() > HARD_MAX_FILES * 2 + 2 {
            return Err(MailDiagnosticError::Capacity);
        }
        private_file(&entry.path(), directory.owner, true)?;
    }
    for (path, id) in temporary {
        let metadata = private_file(&path, directory.owner, true)?;
        if metadata.nlink() == 2 {
            let final_path = directory.path.join(format!("{id}.evidence"));
            let final_metadata = private_file(&final_path, directory.owner, true)?;
            if !same_file(&metadata, &final_metadata) {
                return Err(MailDiagnosticError::Io);
            }
        }
        remove_file(&path)?;
    }
    let mut result = Vec::with_capacity(records.len());
    for path in records {
        let metadata = private_file(&path, directory.owner, false)?;
        if metadata.len() > MAX_FILE_BYTES as u64 {
            return Err(MailDiagnosticError::Capacity);
        }
        result.push(RecordEntry {
            path,
            modified: metadata.modified().map_err(|diagnostic_cause| {
                safe_diagnostics::error(
                    "mooglemail_diagnostics",
                    "read_record_timestamp",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                MailDiagnosticError::Io
            })?,
            size: metadata.len(),
            // 分类只在本次写入确实需要淘汰时才读取，普通追加不额外扫描证据头。
            write_unknown: false,
        });
    }
    result.sort_by_key(|record| record.modified);
    Ok(result)
}

/// 读取公开头首行判断是否为写入未知证据。
/// 无法读取或解析时按“需要保留”处理：分类失败不能成为先删证据的理由，
/// 容量边界仍由后续的统一检查兜底。
pub(super) fn write_unknown_evidence(path: &Path, owner: u32) -> bool {
    let Ok(file) = open_private(path, owner, false) else {
        return true;
    };
    let mut header = Vec::new();
    if file.take((MAX_HEADER_BYTES + 1) as u64).read_to_end(&mut header).is_err() {
        return true;
    }
    let Some(split) = header.iter().position(|byte| *byte == b'\n') else {
        return true;
    };
    let Ok(parsed) = serde_json::from_slice::<EvictionHeader>(&header[..split]) else {
        return true;
    };
    parsed.outcome.failure_code == WRITE_UNKNOWN_FAILURE_CODE
}
