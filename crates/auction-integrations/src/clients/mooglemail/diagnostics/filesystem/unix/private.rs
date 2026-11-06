//! 私有目录与私有文件的创建、打开与校验；权限、属主和链接数都在打开前后核对。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::{
    fs::{self, DirBuilder, File, Metadata, OpenOptions},
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use auction_application::mail::MailDiagnosticError;

use super::path::{normalized_target, same_file};

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
    let metadata = fs::symlink_metadata(&path).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "inspect_directory",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
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
    let metadata = fs::symlink_metadata(path).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "inspect_file",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
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

pub(super) fn open_private(
    path: &Path,
    owner: u32,
    write: bool,
) -> Result<File, MailDiagnosticError> {
    let before = private_file(path, owner, false)?;
    let file =
        OpenOptions::new().read(true).write(write).open(path).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "open_file",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?;
    let opened = file.metadata().map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "inspect_open_file",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    let after = private_file(path, owner, false)?;
    if !same_file(&before, &opened) || !same_file(&opened, &after) {
        return Err(MailDiagnosticError::Io);
    }
    Ok(file)
}
