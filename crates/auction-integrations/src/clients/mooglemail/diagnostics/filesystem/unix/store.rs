//! 证据落盘与读取：临时文件、硬链接无覆盖发布，以及目录内淘汰。

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    time::SystemTime,
};

use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::mail::MailDiagnosticError;
use uuid::Uuid;

use super::super::{
    MAX_FILE_BYTES, MoogleMailDiagnosticArchiveConfig, RecordEntry, eviction_order, eviction_plan,
};
use super::{
    lock::locked_directory,
    path::{normalized_target, same_file},
    private::open_private,
    records::{HARD_MAX_FILES, collect_records, remove_file, write_unknown_evidence},
};
use crate::safe_diagnostics;

pub(in super::super) fn persist(
    config: &MoogleMailDiagnosticArchiveConfig,
    id: Uuid,
    bytes: &[u8],
) -> Result<(), MailDiagnosticError> {
    let directory = locked_directory(&config.directory, true)?;
    let destination = directory.path.join(format!("{id}.evidence"));
    match fs::symlink_metadata(&destination) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        _ => return Err(MailDiagnosticError::Io),
    }
    let mut records = collect_records(&directory)?;
    let now = SystemTime::now();
    let incoming = bytes.len() as u64;
    let plan = |records: &[RecordEntry]| {
        eviction_plan(
            records,
            now,
            incoming,
            config.retention,
            config.maximum_total_bytes,
            HARD_MAX_FILES,
        )
    };
    // 普通追加不需要读取证据头：没有淘汰需求时直接用修改时间顺序。
    let mut evicted = plan(&records)?;
    if evicted > 0 {
        for record in &mut records {
            record.write_unknown = write_unknown_evidence(&record.path, directory.owner);
        }
        records = eviction_order(records).into();
        evicted = plan(&records)?;
    }
    for record in records.iter().take(evicted) {
        remove_file(&record.path)?;
    }
    let temporary = directory.path.join(format!(".{id}.tmp"));
    publish_private(&temporary, &destination, bytes, Some(directory.owner))
}

pub(in super::super) fn read(
    config: &MoogleMailDiagnosticArchiveConfig,
    id: Uuid,
) -> Result<Vec<u8>, MailDiagnosticError> {
    let directory = locked_directory(&config.directory, false)?;
    let path = directory.path.join(format!("{id}.evidence"));
    if fs::symlink_metadata(&path).is_err_and(|error| error.kind() == io::ErrorKind::NotFound) {
        return Err(MailDiagnosticError::NotFound);
    }
    let file = open_private(&path, directory.owner, false)?;
    if file
        .metadata()
        .map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "inspect_record",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?
        .len()
        > MAX_FILE_BYTES as u64
    {
        return Err(MailDiagnosticError::PayloadTooLarge);
    }
    let mut bytes = Vec::new();
    file.take((MAX_FILE_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "read_record",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(MailDiagnosticError::PayloadTooLarge);
    }
    Ok(bytes)
}

fn publish_private(
    temporary: &Path,
    destination: &Path,
    bytes: &[u8],
    owner: Option<u32>,
) -> Result<(), MailDiagnosticError> {
    let mut file =
        OpenOptions::new().write(true).create_new(true).mode(0o600).open(temporary).map_err(
            |diagnostic_cause| {
                safe_diagnostics::error(
                    "mooglemail_diagnostics",
                    "create_private_file",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                MailDiagnosticError::Io
            },
        )?;
    let metadata = file.metadata().map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "inspect_private_file",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    let mut published = false;
    let result = (|| {
        if owner.is_some_and(|owner| metadata.uid() != owner)
            || metadata.permissions().mode() & 0o7777 != 0o600
        {
            return Err(MailDiagnosticError::Io);
        }
        file.write_all(bytes).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "write_record",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?;
        file.sync_all().map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "sync_record",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?;
        // hard_link 在目标已存在时失败，保证不会覆盖已发布证据或用户的导出文件。
        fs::hard_link(temporary, destination).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "publish_record",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?;
        published = true;
        remove_file(temporary)?;
        let parent = destination.parent().ok_or(MailDiagnosticError::Io)?;
        File::open(parent).and_then(|file| file.sync_all()).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "sync_parent",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })
    })();
    if result.is_err() {
        for path in [Some(temporary), published.then_some(destination)].into_iter().flatten() {
            if fs::symlink_metadata(path).is_ok_and(|current| same_file(&current, &metadata)) {
                let _ = fs::remove_file(path);
            }
        }
    }
    result
}

pub(in super::super) fn write_private_export(
    output: &Path,
    bytes: &[u8],
) -> Result<(), MailDiagnosticError> {
    let output = normalized_target(output)?;
    let parent = output.parent().ok_or(MailDiagnosticError::InvalidContext)?;
    let temporary = parent.join(format!(".auction-mail-export-{}.tmp", Uuid::new_v4()));
    publish_private(&temporary, &output, bytes, None)
}
