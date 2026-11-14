//! 跨进程目录锁：私有 `.lock` 文件加非阻塞 flock，并用探针文件核对预存目录属主。

use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::mail::MailDiagnosticError;
use uuid::Uuid;

use super::{
    path::same_file,
    private::{open_private, private_directory, private_file},
};
use crate::safe_diagnostics;

pub(super) struct LockedDirectory {
    pub(super) path: PathBuf,
    pub(super) owner: u32,
    _lock: File,
}

pub(super) fn locked_directory(
    target: &Path,
    create: bool,
) -> Result<LockedDirectory, MailDiagnosticError> {
    let (path, directory_metadata) = private_directory(target, create)?;
    let owner = directory_metadata.uid();
    let lock_path = path.join(".lock");
    let lock = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&lock_path)
    {
        Ok(file) => {
            let opened = file.metadata().map_err(|diagnostic_cause| {
                safe_diagnostics::error(
                    "mooglemail_diagnostics",
                    "inspect_lock_file",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                MailDiagnosticError::Io
            })?;
            if !private_file(&lock_path, owner, false)
                .is_ok_and(|current| same_file(&current, &opened))
            {
                if fs::symlink_metadata(&lock_path)
                    .is_ok_and(|current| same_file(&current, &opened))
                {
                    let _ = fs::remove_file(&lock_path);
                }
                return Err(MailDiagnosticError::Io);
            }
            file
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            open_private(&lock_path, owner, true)?
        }
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
    // 不在后台阻塞任务中无限等待其他进程；锁繁忙也是可观测的留证失败。
    lock.try_lock().map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "lock_directory",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    let current = fs::symlink_metadata(&path).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "inspect_locked_directory",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    if !same_file(&directory_metadata, &current) {
        return Err(MailDiagnosticError::Io);
    }

    // 新建的空私有文件由当前有效账号拥有，用它核对预存目录 owner，避免依赖环境变量或新增 FFI。
    let probe_path = path.join(format!(".{}.tmp", Uuid::new_v4()));
    let probe =
        OpenOptions::new().write(true).create_new(true).mode(0o600).open(&probe_path).map_err(
            |diagnostic_cause| {
                safe_diagnostics::error(
                    "mooglemail_diagnostics",
                    "create_probe",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                MailDiagnosticError::Io
            },
        )?;
    let probe_metadata = probe.metadata().map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "inspect_probe",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    });
    drop(probe);
    fs::remove_file(&probe_path).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "remove_probe",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    if probe_metadata?.uid() != owner {
        return Err(MailDiagnosticError::Io);
    }
    Ok(LockedDirectory { path, owner, _lock: lock })
}
