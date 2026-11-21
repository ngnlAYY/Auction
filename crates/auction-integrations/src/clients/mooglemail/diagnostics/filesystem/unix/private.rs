//! 私有目录与私有文件的创建、打开与校验；权限、属主和链接数均按元数据核对。

use std::{
    fs::{self, DirBuilder, Metadata},
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::mail::MailDiagnosticError;

use super::path::normalized_target;
use crate::safe_diagnostics;

pub(super) fn private_directory(
    target: &Path,
    create: bool,
) -> Result<(PathBuf, Metadata), MailDiagnosticError> {
    let path = normalized_target(target)?;
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
            let result = DirBuilder::new().mode(0o700).create(&path);
            if result.as_ref().is_err_and(|error| error.kind() != io::ErrorKind::AlreadyExists) {
                return Err(MailDiagnosticError::Io);
            }
        }
        Err(diagnostic_cause) => {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "read_directory",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return Err(MailDiagnosticError::NotFound);
        }
    }
    let metadata = fs::symlink_metadata(&path).map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "inspect_directory",
        || MailDiagnosticError::Io,
    ))?;
    // Archive contents are trusted only when the service account owns their private directory.
    // SAFETY: geteuid takes no pointers and does not access Rust-managed memory.
    let owner = unsafe { libc::geteuid() };
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != owner
        || metadata.permissions().mode() & 0o7777 != 0o700
    {
        return Err(MailDiagnosticError::Io);
    }
    Ok((path, metadata))
}

pub(super) fn private_file(
    path: &Path,
    owner: u32,
    allow_two_links: bool,
) -> Result<Metadata, MailDiagnosticError> {
    let metadata = fs::symlink_metadata(path).map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "inspect_file",
        || MailDiagnosticError::Io,
    ))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != owner
        || metadata.permissions().mode() & 0o7777 != 0o600
        || !(metadata.nlink() == 1 || allow_two_links && metadata.nlink() == 2)
    {
        return Err(MailDiagnosticError::Io);
    }
    Ok(metadata)
}
