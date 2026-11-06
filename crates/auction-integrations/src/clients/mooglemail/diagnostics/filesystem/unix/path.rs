//! 路径规范化与文件身份比较；父目录逐级拒绝符号链接。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::{
    fs::{self, Metadata},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use auction_application::mail::MailDiagnosticError;

/// 目录配置可相对于 Worker TOML；逐级拒绝符号链接后才规范化父目录。
pub(super) fn normalized_target(target: &Path) -> Result<PathBuf, MailDiagnosticError> {
    let target = std::path::absolute(target).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "resolve_absolute_path",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    let name = target.file_name().ok_or(MailDiagnosticError::InvalidContext)?;
    let parent = target.parent().ok_or(MailDiagnosticError::InvalidContext)?;
    let mut prefix = PathBuf::new();
    for component in parent.components() {
        prefix.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&prefix).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "inspect_parent",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(MailDiagnosticError::Io);
        }
    }
    let parent = fs::canonicalize(parent).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "canonicalize_parent",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    Ok(parent.join(name))
}

pub(super) fn same_file(first: &Metadata, second: &Metadata) -> bool {
    first.dev() == second.dev() && first.ino() == second.ino()
}
