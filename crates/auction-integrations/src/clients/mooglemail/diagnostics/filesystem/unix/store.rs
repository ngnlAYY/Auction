//! 证据落盘与读取：临时文件、硬链接无覆盖发布，以及目录内淘汰。

use std::{
    fs::File,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    time::SystemTime,
};

use auction_application::mail::MailDiagnosticError;
use uuid::Uuid;

use super::super::{MAX_FILE_BYTES, RecordEntry, eviction_order, eviction_plan};
use super::{
    lock::locked_directory,
    path::{linkat, normalized_target, open_directory_chain, openat_file, same_file, unlinkat},
    records::{HARD_MAX_FILES, collect_records, remove_file, write_unknown_evidence},
};
use crate::clients::mooglemail::diagnostics::MoogleMailDiagnosticArchiveConfig;
use crate::safe_diagnostics;

pub(in super::super) fn persist(
    config: &MoogleMailDiagnosticArchiveConfig,
    id: Uuid,
    bytes: &[u8],
) -> Result<(), MailDiagnosticError> {
    let directory = locked_directory(&config.directory, true)?;
    let destination_name = format!("{id}.evidence");
    match openat_file(
        &directory.directory,
        &destination_name,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        0,
    ) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
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
            record.write_unknown = write_unknown_evidence(&directory, &record.path);
        }
        records = eviction_order(records).into();
        evicted = plan(&records)?;
    }
    for record in records.iter().take(evicted) {
        remove_file(&directory, &record.path)?;
    }
    if evicted > 0 {
        directory.directory.sync_all().map_err(safe_diagnostics::error_map(
            "mooglemail_diagnostics",
            "sync_evicted_records",
            || MailDiagnosticError::Io,
        ))?;
    }
    let temporary_name = format!(".{}.tmp", Uuid::new_v4());
    publish_private(
        &directory.directory,
        &temporary_name,
        &destination_name,
        bytes,
        Some(directory.owner),
    )
}

pub(in super::super) fn read(
    config: &MoogleMailDiagnosticArchiveConfig,
    id: Uuid,
) -> Result<Vec<u8>, MailDiagnosticError> {
    let directory = locked_directory(&config.directory, false)?;
    let name = format!("{id}.evidence");
    let file = match openat_file(
        &directory.directory,
        &name,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        0,
    ) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(MailDiagnosticError::NotFound);
        }
        Err(error) => {
            return Err(safe_diagnostics::error_map(
                "mooglemail_diagnostics",
                "open_record",
                || MailDiagnosticError::Io,
            )(error));
        }
    };
    let metadata = file.metadata().map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "inspect_record",
        || MailDiagnosticError::Io,
    ))?;
    if !metadata.is_file()
        || metadata.uid() != directory.owner
        || metadata.permissions().mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(MailDiagnosticError::Io);
    }
    if metadata.len() > MAX_FILE_BYTES as u64 {
        return Err(MailDiagnosticError::PayloadTooLarge);
    }
    let mut bytes = Vec::new();
    file.take((MAX_FILE_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(
        safe_diagnostics::error_map("mooglemail_diagnostics", "read_record", || {
            MailDiagnosticError::Io
        }),
    )?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(MailDiagnosticError::PayloadTooLarge);
    }
    Ok(bytes)
}

fn publish_private(
    directory: &File,
    temporary: impl AsRef<std::ffi::OsStr>,
    destination: impl AsRef<std::ffi::OsStr>,
    bytes: &[u8],
    owner: Option<u32>,
) -> Result<(), MailDiagnosticError> {
    let temporary = temporary.as_ref();
    let destination = destination.as_ref();
    let mut file = openat_file(
        directory,
        temporary,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        0o600,
    )
    .map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "create_private_file",
        || MailDiagnosticError::Io,
    ))?;
    let metadata = file.metadata().map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "inspect_private_file",
        || MailDiagnosticError::Io,
    ))?;
    let mut published = false;
    let result = (|| {
        if owner.is_some_and(|owner| metadata.uid() != owner)
            || metadata.permissions().mode() & 0o7777 != 0o600
        {
            return Err(MailDiagnosticError::Io);
        }
        file.write_all(bytes).map_err(safe_diagnostics::error_map(
            "mooglemail_diagnostics",
            "write_record",
            || MailDiagnosticError::Io,
        ))?;
        file.sync_all().map_err(safe_diagnostics::error_map(
            "mooglemail_diagnostics",
            "sync_record",
            || MailDiagnosticError::Io,
        ))?;
        linkat(directory, temporary, destination).map_err(safe_diagnostics::error_map(
            "mooglemail_diagnostics",
            "publish_record",
            || MailDiagnosticError::Io,
        ))?;
        published = true;
        let published_file = openat_file(
            directory,
            destination,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
        )
        .map_err(safe_diagnostics::error_map(
            "mooglemail_diagnostics",
            "verify_record_link",
            || MailDiagnosticError::Io,
        ))?;
        let published_metadata = published_file.metadata().map_err(safe_diagnostics::error_map(
            "mooglemail_diagnostics",
            "inspect_published_record",
            || MailDiagnosticError::Io,
        ))?;
        if !same_file(&metadata, &published_metadata)
            || published_metadata.nlink() != 2
            || published_metadata.uid() != metadata.uid()
            || published_metadata.permissions().mode() & 0o7777 != 0o600
        {
            return Err(MailDiagnosticError::Io);
        }
        drop(published_file);
        directory.sync_all().map_err(safe_diagnostics::error_map(
            "mooglemail_diagnostics",
            "sync_parent_after_link",
            || MailDiagnosticError::Io,
        ))?;
        unlinkat(directory, temporary).map_err(safe_diagnostics::error_map(
            "mooglemail_diagnostics",
            "remove_temporary_record",
            || MailDiagnosticError::Io,
        ))?;
        directory.sync_all().map_err(safe_diagnostics::error_map(
            "mooglemail_diagnostics",
            "sync_parent",
            || MailDiagnosticError::Io,
        ))
    })();
    if result.is_err() {
        for name in [Some(temporary), published.then_some(destination)].into_iter().flatten() {
            if let Ok(current) =
                openat_file(directory, name, libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0)
                && current.metadata().is_ok_and(|current| same_file(&current, &metadata))
            {
                drop(current);
                let _ = unlinkat(directory, name);
            }
        }
        let _ = directory.sync_all();
    }
    result
}

pub(in super::super) fn write_private_export(
    output: &Path,
    bytes: &[u8],
) -> Result<(), MailDiagnosticError> {
    let output = normalized_target(output)?;
    let parent = output.parent().ok_or(MailDiagnosticError::InvalidContext)?;
    let destination = output.file_name().ok_or(MailDiagnosticError::InvalidContext)?;
    let chain = open_directory_chain(parent).map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "pin_export_directory_chain",
        || MailDiagnosticError::Io,
    ))?;
    let directory = chain.last().ok_or(MailDiagnosticError::Io)?;
    let temporary = format!(".auction-mail-export-{}.tmp", Uuid::new_v4());
    publish_private(directory, &temporary, destination, bytes, None)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        mem::DropGuard,
        os::unix::fs::{DirBuilderExt, PermissionsExt},
    };

    use uuid::Uuid;

    use super::{locked_directory, publish_private};

    #[test]
    fn pinned_directory_survives_writable_ancestor_replacement() {
        let root =
            std::env::temp_dir().join(format!("auction-diagnostic-pinned-{}", Uuid::new_v4()));
        fs::create_dir(&root).expect("create test root");
        let _guard = DropGuard::new(root.clone(), |path| {
            let _ = fs::remove_dir_all(path);
        });
        let ancestor = root.join("ancestor");
        fs::create_dir(&ancestor).expect("create ancestor");
        let target = ancestor.join("archive");
        let pinned = locked_directory(&target, true).expect("lock private archive");
        let moved = root.join("moved-ancestor");
        fs::rename(&ancestor, &moved).expect("move original ancestor");
        fs::create_dir(&ancestor).expect("recreate writable ancestor");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(ancestor.join("archive"))
            .expect("replace writable ancestor path");

        publish_private(&pinned.directory, ".test.tmp", "record.evidence", b"secret", None)
            .expect("publish through held descriptor");

        assert!(moved.join("archive/record.evidence").is_file());
        assert!(!ancestor.join("archive/record.evidence").exists());
        assert_eq!(
            fs::metadata(moved.join("archive/record.evidence"))
                .expect("published file")
                .permissions()
                .mode()
                & 0o7777,
            0o600
        );
    }
}
