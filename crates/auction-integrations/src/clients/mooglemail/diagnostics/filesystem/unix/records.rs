//! 证据目录清单：严格命名识别、临时文件清理和写入未知证据分类。

use std::{
    fs,
    io::Read,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::Path,
};

use auction_application::mail::MailDiagnosticError;
use uuid::Uuid;

use super::super::super::payload::{self, PublicHeader};
use super::super::{
    EvictionHeader, MAX_FILE_BYTES, MAX_HEADER_BYTES, RecordEntry, WRITE_UNKNOWN_FAILURE_CODE,
};
use super::{
    lock::LockedDirectory,
    path::{openat_file, same_file, unlinkat},
};
use crate::safe_diagnostics;

/// 目录内受管文件数（证据 + 临时）的硬上限，超出视为容量异常。
pub(super) const HARD_MAX_FILES: usize = 1024;

pub(super) fn managed_name(name: &str, temporary: bool) -> Option<Uuid> {
    let raw = if temporary {
        name.strip_circumfix('.', ".tmp")?
    } else {
        name.strip_suffix(".evidence")?
    };
    let id = Uuid::parse_str(raw).ok()?;
    (!id.is_nil() && id.to_string() == raw).then_some(id)
}

pub(super) fn remove_file(
    directory: &LockedDirectory,
    path: &Path,
) -> Result<(), MailDiagnosticError> {
    let name = path.file_name().and_then(|name| name.to_str()).ok_or(MailDiagnosticError::Io)?;
    let file = openat_file(
        &directory.directory,
        name,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        0,
    )
    .map_err(safe_diagnostics::error_map("mooglemail_diagnostics", "inspect_file", || {
        MailDiagnosticError::Io
    }))?;
    validate_file(&file, directory.owner, false)?;
    let metadata = file.metadata().map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "inspect_file",
        || MailDiagnosticError::Io,
    ))?;
    unlink_file_if_same(directory, name, &metadata, false)
}

fn private_file_at(
    directory: &LockedDirectory,
    name: &str,
    allow_two_links: bool,
) -> Result<fs::Metadata, MailDiagnosticError> {
    let file = openat_file(
        &directory.directory,
        name,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        0,
    )
    .map_err(safe_diagnostics::error_map("mooglemail_diagnostics", "open_file", || {
        MailDiagnosticError::Io
    }))?;
    validate_file(&file, directory.owner, allow_two_links)?;
    file.metadata().map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "inspect_open_file",
        || MailDiagnosticError::Io,
    ))
}

fn unlink_file_if_same(
    directory: &LockedDirectory,
    name: &str,
    expected: &fs::Metadata,
    allow_two_links: bool,
) -> Result<(), MailDiagnosticError> {
    let current = private_file_at(directory, name, allow_two_links)?;
    if !same_file(expected, &current) {
        return Err(MailDiagnosticError::Io);
    }
    unlinkat(&directory.directory, name).map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "remove_record",
        || MailDiagnosticError::Io,
    ))
}

fn validate_file(
    file: &fs::File,
    owner: u32,
    allow_two_links: bool,
) -> Result<(), MailDiagnosticError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = file.metadata().map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "inspect_open_file",
        || MailDiagnosticError::Io,
    ))?;
    if !metadata.is_file()
        || metadata.uid() != owner
        || metadata.permissions().mode() & 0o7777 != 0o600
        || !(metadata.nlink() == 1 || allow_two_links && metadata.nlink() == 2)
    {
        return Err(MailDiagnosticError::Io);
    }
    Ok(())
}

fn validated_header_id(
    directory: &LockedDirectory,
    name: &str,
    expected: &fs::Metadata,
) -> Result<Uuid, MailDiagnosticError> {
    let mut file = openat_file(
        &directory.directory,
        name,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        0,
    )
    .map_err(safe_diagnostics::error_map("mooglemail_diagnostics", "open_record", || {
        MailDiagnosticError::Io
    }))?;
    validate_file(&file, directory.owner, true)?;
    if !same_file(
        expected,
        &file.metadata().map_err(safe_diagnostics::error_map(
            "mooglemail_diagnostics",
            "inspect_record_header",
            || MailDiagnosticError::Io,
        ))?,
    ) {
        return Err(MailDiagnosticError::Io);
    }
    let mut header = Vec::new();
    file.by_ref().take((MAX_HEADER_BYTES + 1) as u64).read_to_end(&mut header).map_err(
        safe_diagnostics::error_map("mooglemail_diagnostics", "read_record_header", || {
            MailDiagnosticError::Io
        }),
    )?;
    let split = header
        .iter()
        .position(|byte| *byte == b'\n')
        .filter(|split| *split <= MAX_HEADER_BYTES)
        .ok_or(MailDiagnosticError::Io)?;
    let parsed: PublicHeader = serde_json::from_slice(&header[..split]).map_err(
        safe_diagnostics::error_map("mooglemail_diagnostics", "decode_record_header", || {
            MailDiagnosticError::Io
        }),
    )?;
    payload::validate_header(&parsed, parsed.evidence_id)?;
    Ok(parsed.evidence_id)
}

/// Recover only one hard-link pair whose final name agrees with its validated evidence header.
pub(super) fn collect_records(
    directory: &LockedDirectory,
) -> Result<Vec<RecordEntry>, MailDiagnosticError> {
    let mut temporary = Vec::new();
    let mut records = Vec::new();
    let entries = fs::read_dir(format!("/dev/fd/{}", directory.directory.as_raw_fd())).map_err(
        safe_diagnostics::error_map("mooglemail_diagnostics", "list_records", || {
            MailDiagnosticError::Io
        }),
    )?;
    for entry in entries {
        let entry = entry.map_err(safe_diagnostics::error_map(
            "mooglemail_diagnostics",
            "read_record_entry",
            || MailDiagnosticError::Io,
        ))?;
        let Ok(name) = entry.file_name().into_string() else {
            return Err(MailDiagnosticError::Io);
        };
        if name == ".lock" {
            private_file_at(directory, &name, false)?;
            continue;
        }
        if let Some(id) = managed_name(&name, true) {
            private_file_at(directory, &name, true)?;
            temporary.push((name, id));
        } else if managed_name(&name, false).is_some() {
            private_file_at(directory, &name, true)?;
            records.push(name);
        } else {
            return Err(MailDiagnosticError::Io);
        }
        if temporary.len() + records.len() > HARD_MAX_FILES * 2 + 2 {
            return Err(MailDiagnosticError::Capacity);
        }
    }

    let had_temporary = !temporary.is_empty();
    for (temporary_name, _) in temporary {
        let metadata = private_file_at(directory, &temporary_name, true)?;
        if metadata.nlink() == 2 {
            let mut matching = None;
            for final_name in &records {
                let final_metadata = private_file_at(directory, final_name, true)?;
                if same_file(&metadata, &final_metadata)
                    && matching.replace(final_name.as_str()).is_some()
                {
                    return Err(MailDiagnosticError::Io);
                }
            }
            let final_name = matching.ok_or(MailDiagnosticError::Io)?;
            let filename_id = managed_name(final_name, false).ok_or(MailDiagnosticError::Io)?;
            if validated_header_id(directory, &temporary_name, &metadata)? != filename_id {
                return Err(MailDiagnosticError::Io);
            }
        }
        unlink_file_if_same(directory, &temporary_name, &metadata, true)?;
    }
    if had_temporary {
        directory.directory.sync_all().map_err(safe_diagnostics::error_map(
            "mooglemail_diagnostics",
            "sync_temporary_cleanup",
            || MailDiagnosticError::Io,
        ))?;
    }

    let mut result = Vec::with_capacity(records.len());
    for name in records {
        let metadata = private_file_at(directory, &name, false)?;
        if metadata.len() > MAX_FILE_BYTES as u64 {
            return Err(MailDiagnosticError::Capacity);
        }
        result.push(RecordEntry {
            path: directory.path.join(name),
            modified: metadata.modified().map_err(safe_diagnostics::error_map(
                "mooglemail_diagnostics",
                "read_record_timestamp",
                || MailDiagnosticError::Io,
            ))?,
            size: metadata.len(),
            write_unknown: false,
        });
    }
    result.sort_by_key(|record| record.modified);
    Ok(result)
}

pub(super) fn write_unknown_evidence(directory: &LockedDirectory, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    let Ok(file) = openat_file(
        &directory.directory,
        name,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        0,
    ) else {
        return true;
    };
    if validate_file(&file, directory.owner, false).is_err() {
        return true;
    }
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

#[cfg(test)]
mod tests {
    use std::{fs, io::Write, mem::DropGuard};

    use auction_application::mail::MailDiagnosticError;
    use uuid::Uuid;

    use super::collect_records;
    use crate::clients::mooglemail::diagnostics::payload::{
        ContextWire, OutcomeWire, PublicHeader,
    };

    fn test_header(id: Uuid) -> PublicHeader {
        PublicHeader {
            version: 2,
            evidence_id: id,
            key_source: "mail_diagnostics".into(),
            key_id: "test_key".into(),
            context: ContextWire {
                stage: "transport".into(),
                operation: "diagnostic_test".into(),
                world_id: "main".into(),
                mailbox_key: None,
                external_mail_id: None,
                external_equipment_id: None,
                attachment_position: None,
            },
            outcome: OutcomeWire { failure_code: "test_failure".into(), decision_code: None },
            payload_kind: "raw_response".into(),
            payload_bytes: 0,
            sha256: "0".repeat(64),
            body_available: false,
            status: None,
        }
    }

    #[test]
    fn recovers_crash_between_link_and_temporary_unlink_with_distinct_ids() {
        let root =
            std::env::temp_dir().join(format!("auction-diagnostic-recovery-{}", Uuid::new_v4()));
        fs::create_dir(&root).expect("create test root");
        let _guard = DropGuard::new(root.clone(), |path| {
            let _ = fs::remove_dir_all(path);
        });
        let directory = super::super::lock::locked_directory(&root.join("archive"), true)
            .expect("lock archive");
        let evidence_id = Uuid::new_v4();
        let temporary_id = Uuid::new_v4();
        assert_ne!(evidence_id, temporary_id);
        let temporary_name = format!(".{temporary_id}.tmp");
        let final_name = format!("{evidence_id}.evidence");
        let mut bytes = serde_json::to_vec(&test_header(evidence_id)).expect("encode header");
        bytes.push(b'\n');
        let mut file = super::super::path::openat_file(
            &directory.directory,
            &temporary_name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
        .expect("create temporary");
        file.write_all(&bytes).expect("write header");
        file.sync_all().expect("sync temporary");
        super::super::path::linkat(&directory.directory, &temporary_name, &final_name)
            .expect("simulate crash after hard link");

        let records = collect_records(&directory).expect("recover exactly matched pair");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].path.file_name().unwrap().to_str(), Some(final_name.as_str()));
        assert!(
            super::super::path::openat_file(
                &directory.directory,
                &temporary_name,
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0,
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_ambiguous_hard_link_recovery_pair() {
        let root =
            std::env::temp_dir().join(format!("auction-diagnostic-ambiguous-{}", Uuid::new_v4()));
        fs::create_dir(&root).expect("create test root");
        let _guard = DropGuard::new(root.clone(), |path| {
            let _ = fs::remove_dir_all(path);
        });
        let directory = super::super::lock::locked_directory(&root.join("archive"), true)
            .expect("lock archive");
        let evidence_id = Uuid::new_v4();
        let temporary_name = format!(".{}.tmp", Uuid::new_v4());
        let final_name = format!("{evidence_id}.evidence");
        let other_name = format!("{}.evidence", Uuid::new_v4());
        let mut bytes = serde_json::to_vec(&test_header(evidence_id)).expect("encode header");
        bytes.push(b'\n');
        let mut file = super::super::path::openat_file(
            &directory.directory,
            &temporary_name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
        .expect("create temporary");
        file.write_all(&bytes).expect("write header");
        super::super::path::linkat(&directory.directory, &temporary_name, &final_name)
            .expect("create final link");
        super::super::path::linkat(&directory.directory, &temporary_name, &other_name)
            .expect("create ambiguous extra link");

        assert!(matches!(collect_records(&directory), Err(MailDiagnosticError::Io)));
    }
}
