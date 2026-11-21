//! 路径规范化与文件身份比较；父目录逐级拒绝符号链接。

use std::{
    ffi::CString,
    fs::{self, File, Metadata},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
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

/// Open and retain each path component without following symlinks.
pub(super) fn open_directory_chain(path: &Path) -> io::Result<Vec<File>> {
    let mut chain = Vec::new();
    let fd = unsafe {
        libc::open(
            c"/".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    chain.push(unsafe { File::from_raw_fd(fd) });
    for component in path.components() {
        let std::path::Component::Normal(name) = component else {
            if matches!(component, std::path::Component::RootDir) {
                continue;
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "non-canonical directory path",
            ));
        };
        let parent = chain.last().ok_or_else(|| io::Error::other("缺少父目录描述符"))?;
        let child = openat_file(
            parent,
            name,
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
        )?;
        chain.push(child);
    }
    Ok(chain)
}

pub(super) fn openat_file(
    directory: &File,
    name: impl AsRef<std::ffi::OsStr>,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> io::Result<File> {
    let name = cstring(name.as_ref())?;
    let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags, mode) };
    if fd < 0 { Err(io::Error::last_os_error()) } else { Ok(unsafe { File::from_raw_fd(fd) }) }
}

pub(super) fn linkat(
    directory: &File,
    source: impl AsRef<std::ffi::OsStr>,
    destination: impl AsRef<std::ffi::OsStr>,
) -> io::Result<()> {
    let source = cstring(source.as_ref())?;
    let destination = cstring(destination.as_ref())?;
    let result = unsafe {
        libc::linkat(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            destination.as_ptr(),
            0,
        )
    };
    if result == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

pub(super) fn unlinkat(directory: &File, name: impl AsRef<std::ffi::OsStr>) -> io::Result<()> {
    let name = cstring(name.as_ref())?;
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn cstring(name: &std::ffi::OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes()).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("路径在字节 {} 含 NUL", error.nul_position()),
        )
    })
}

pub(super) fn same_file(first: &Metadata, second: &Metadata) -> bool {
    first.dev() == second.dev() && first.ino() == second.ino()
}
