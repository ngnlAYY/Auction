//! Worker control runtime 目录、实例锁和 socket 文件安全边界。

use super::*;

#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(unix)]
pub(super) struct SocketCleanupGuard {
    path: PathBuf,
    identity: FileIdentity,
}

#[cfg(unix)]
impl SocketCleanupGuard {
    pub(super) fn new(path: PathBuf, identity: FileIdentity) -> Self {
        Self { path, identity }
    }
}

#[cfg(unix)]
impl Drop for SocketCleanupGuard {
    fn drop(&mut self) {
        let _ = remove_if_same_socket(&self.path, Some(self.identity));
    }
}

#[cfg(unix)]
pub(super) fn resolve_group_id(group: &str) -> Result<u32, WorkerControlServerError> {
    let encoded = CString::new(group)
        .map_err(|_| WorkerControlServerError::InvalidSecurityConfig("group 包含 NUL"))?;
    // SAFETY: encoded 是 NUL 结尾的不可变 group 名称；返回记录只在本次同步调用内读取。
    let record = unsafe { libc::getgrnam(encoded.as_ptr()) };
    if record.is_null() {
        return Err(WorkerControlServerError::UnknownGroup);
    }
    // SAFETY: 上一步已排除空指针，gr_gid 是 libc group 记录中的值字段。
    Ok(unsafe { (*record).gr_gid })
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: geteuid 不接收指针，也不访问 Rust 管理的内存。
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
pub(super) fn validate_runtime_directory(
    path: &Path,
    group_id: u32,
) -> Result<(), WorkerControlServerError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let resolved = std::fs::canonicalize(path).map_err(WorkerControlServerError::Io)?;
    if resolved != path {
        return Err(WorkerControlServerError::UnsafeRuntimeDirectory);
    }
    let metadata = std::fs::symlink_metadata(path).map_err(WorkerControlServerError::Io)?;
    let mode = metadata.permissions().mode() & 0o777;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != current_uid()
        || mode & 0o002 != 0
        || mode & 0o020 != 0 && metadata.gid() != group_id
    {
        return Err(WorkerControlServerError::UnsafeRuntimeDirectory);
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn acquire_instance_lock(socket_path: &Path) -> Result<File, WorkerControlServerError> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let mut lock_name = socket_path.as_os_str().as_bytes().to_vec();
    lock_name.extend_from_slice(b".lock");
    let lock_path = PathBuf::from(std::ffi::OsString::from_vec(lock_name));
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&lock_path)
        .map_err(WorkerControlServerError::Io)?;
    let metadata = lock.metadata().map_err(WorkerControlServerError::Io)?;
    let path_metadata =
        std::fs::symlink_metadata(&lock_path).map_err(WorkerControlServerError::Io)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o777 != 0o600
        || path_metadata.file_type().is_symlink()
        || path_metadata.dev() != metadata.dev()
        || path_metadata.ino() != metadata.ino()
    {
        return Err(WorkerControlServerError::InvalidSecurityConfig(
            "实例锁必须是当前用户拥有的 0600 单链接普通文件",
        ));
    }
    // SAFETY: fd 来自仍由 lock 持有的普通文件；flock 不接管 fd，也不修改 Rust 内存。
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        let raw_error = error.raw_os_error();
        if raw_error == Some(libc::EWOULDBLOCK) || raw_error == Some(libc::EAGAIN) {
            return Err(WorkerControlServerError::ActiveSocket);
        }
        return Err(WorkerControlServerError::Io(error));
    }
    Ok(lock)
}

#[cfg(unix)]
pub(super) async fn prepare_socket_path(path: &Path) -> Result<(), WorkerControlServerError> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(WorkerControlServerError::Io(error)),
    };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != current_uid()
    {
        return Err(WorkerControlServerError::SocketPathOccupied);
    }
    let identity = FileIdentity { device: metadata.dev(), inode: metadata.ino() };
    match tokio::time::timeout(Duration::from_millis(100), tokio::net::UnixStream::connect(path))
        .await
    {
        Ok(Ok(stream)) => {
            drop(stream);
            Err(WorkerControlServerError::ActiveSocket)
        }
        Ok(Err(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            ) =>
        {
            remove_if_same_socket(path, Some(identity))?;
            Ok(())
        }
        Ok(Err(_)) | Err(_) => Err(WorkerControlServerError::UnverifiableSocket),
    }
}

#[cfg(unix)]
pub(super) fn secure_bound_socket(
    path: &Path,
    group_id: u32,
    identity: FileIdentity,
) -> Result<(), WorkerControlServerError> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

    let encoded = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| WorkerControlServerError::InvalidSocketPath)?;
    // SAFETY: encoded 是已校验路径的 NUL 结尾副本，uid/gid 均为本地内核标识。
    if unsafe { libc::chown(encoded.as_ptr(), current_uid(), group_id) } != 0 {
        return Err(WorkerControlServerError::Io(std::io::Error::last_os_error()));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))
        .map_err(WorkerControlServerError::Io)?;
    let metadata = std::fs::symlink_metadata(path).map_err(WorkerControlServerError::Io)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != current_uid()
        || metadata.gid() != group_id
        || metadata.permissions().mode() & 0o777 != 0o660
        || (FileIdentity { device: metadata.dev(), inode: metadata.ino() }) != identity
    {
        return Err(WorkerControlServerError::InvalidSecurityConfig(
            "绑定后的 socket owner/group/权限或身份不符合合同",
        ));
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn socket_identity(path: &Path) -> Result<FileIdentity, WorkerControlServerError> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata = std::fs::symlink_metadata(path).map_err(WorkerControlServerError::Io)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != current_uid()
    {
        return Err(WorkerControlServerError::SocketPathOccupied);
    }
    Ok(FileIdentity { device: metadata.dev(), inode: metadata.ino() })
}

#[cfg(unix)]
pub(super) fn remove_if_same_socket(
    path: &Path,
    expected: Option<FileIdentity>,
) -> Result<(), WorkerControlServerError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    match socket_identity(path) {
        Ok(current) if current == expected => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(WorkerControlServerError::Io(error)),
        },
        Ok(_) | Err(WorkerControlServerError::SocketPathOccupied) => Ok(()),
        Err(WorkerControlServerError::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}
