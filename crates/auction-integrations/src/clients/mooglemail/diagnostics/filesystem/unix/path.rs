//! 路径规范化与文件身份比较；父目录逐级拒绝符号链接。

use std::{
    fs::{self, Metadata},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use auction_application::mail::MailDiagnosticError;

use crate::safe_diagnostics;

/// 目录配置可相对于 Worker TOML；逐级拒绝符号链接后才规范化父目录。
pub(super) fn normalized_target(target: &Path) -> Result<PathBuf, MailDiagnosticError> {
    let target = std::path::absolute(target).map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "resolve_absolute_path",
        || MailDiagnosticError::Io,
    ))?;
    let name = target.file_name().ok_or(MailDiagnosticError::InvalidContext)?;
    let parent = target.parent().ok_or(MailDiagnosticError::InvalidContext)?;
    let mut prefix = PathBuf::new();
    for component in parent.components() {
        prefix.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&prefix).map_err(safe_diagnostics::error_map(
            "mooglemail_diagnostics",
            "inspect_parent",
            || MailDiagnosticError::Io,
        ))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(MailDiagnosticError::Io);
        }
    }
    let parent = fs::canonicalize(parent).map_err(safe_diagnostics::error_map(
        "mooglemail_diagnostics",
        "canonicalize_parent",
        || MailDiagnosticError::Io,
    ))?;
    Ok(parent.join(name))
}

pub(super) fn same_file(first: &Metadata, second: &Metadata) -> bool {
    first.dev() == second.dev() && first.ino() == second.ino()
}
