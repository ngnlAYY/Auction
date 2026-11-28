//! 跨进程目录锁：私有 `.lock` 文件加非阻塞 flock，并用探针文件核对预存目录属主。

use std::{
    fs::File,
    io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::mail::MailDiagnosticError;
use uuid::Uuid;

use super::{path::same_file, private::private_directory};
use crate::safe_diagnostics;

pub(super) struct LockedDirectory {
    pub(super) path: PathBuf,
    pub(super) owner: u32,
    pub(super) directory: File,
    _chain: Vec<File>,
    _lock: File,
}

pub(super) fn locked_directory(
    target: &Path,
    create: bool,
) -> Result<LockedDirectory, MailDiagnosticError> {
    let (path, directory_metadata) = private_directory(target, create)?;
    let owner = directory_metadata.uid();
    let chain = super::path::open_directory_chain(&path).map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "pin_directory_chain",
        || MailDiagnosticError::Io,
    ))?;
    let directory = chain.last().ok_or(MailDiagnosticError::Io)?;
    let pinned_metadata = directory.metadata().map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "inspect_pinned_directory",
        || MailDiagnosticError::Io,
    ))?;
    if !same_file(&directory_metadata, &pinned_metadata)
        || pinned_metadata.uid() != owner
        || pinned_metadata.permissions().mode() & 0o7777 != 0o700
    {
        return Err(MailDiagnosticError::Io);
    }
    let directory = directory.try_clone().map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "duplicate_directory_descriptor",
        || MailDiagnosticError::Io,
    ))?;

    let lock = match super::path::openat_file(
        &directory,
        ".lock",
        libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        0o600,
    ) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => super::path::openat_file(
            &directory,
            ".lock",
            libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
        )
        .map_err(safe_diagnostics::error_map("mooglemail_diagnostics", "open_lock_file", || {
            MailDiagnosticError::Io
        }))?,
        Err(diagnostic_cause) => {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "create_lock_file",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return Err(MailDiagnosticError::Io);
        }
    };
    let lock_metadata = lock.metadata().map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "inspect_lock_file",
        || MailDiagnosticError::Io,
    ))?;
    if !lock_metadata.is_file()
        || lock_metadata.uid() != owner
        || lock_metadata.permissions().mode() & 0o7777 != 0o600
        || lock_metadata.nlink() != 1
    {
        return Err(MailDiagnosticError::Io);
    }
    let current_lock = super::path::openat_file(
        &directory,
        ".lock",
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        0,
    )
    .map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "verify_lock_file",
        || MailDiagnosticError::Io,
    ))?;
    if !same_file(
        &lock_metadata,
        &current_lock.metadata().map_err(safe_diagnostics::error_map(
            "mooglemail_diagnostics",
            "inspect_current_lock_file",
            || MailDiagnosticError::Io,
        ))?,
    ) {
        return Err(MailDiagnosticError::Io);
    }
    drop(current_lock);
    lock.try_lock().map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "lock_directory",
        || MailDiagnosticError::Io,
    ))?;

    // Probe ownership using descriptor-relative creation and cleanup.
    // 探针名不落入受管的 `.<uuid>.tmp` 临时证据模式：unlink 失败残留会作为未识别目录项
    // 暴露给保留快照，而不是被静默计为在途临时文件。
    let probe_name = format!(".lock-probe-{}", Uuid::new_v4());
    let probe = super::path::openat_file(
        &directory,
        &probe_name,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        0o600,
    )
    .map_err(safe_diagnostics::error_map("mooglemail_diagnostics", "create_probe", || {
        MailDiagnosticError::Io
    }))?;
    let probe_metadata = probe.metadata().map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "inspect_probe",
        || MailDiagnosticError::Io,
    ));
    drop(probe);
    super::path::unlinkat(&directory, &probe_name).map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "remove_probe",
        || MailDiagnosticError::Io,
    ))?;
    directory.sync_all().map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "sync_private_directory",
        || MailDiagnosticError::Io,
    ))?;
    if probe_metadata?.uid() != owner {
        return Err(MailDiagnosticError::Io);
    }
    Ok(LockedDirectory { path, owner, directory, _chain: chain, _lock: lock })
}
