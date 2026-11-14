//! Worker control runtime 目录、实例锁和 socket 文件安全边界。

use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::control::transport::WorkerControlServerError;

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
    let mut buffer = vec![0_u8; 1024];
    loop {
        let mut record = std::mem::MaybeUninit::<libc::group>::uninit();
        let mut result = std::ptr::null_mut();
        // SAFETY: 输入以 NUL 结尾；记录、缓冲和结果指针均由本次调用独占，
        // 不使用会被其他线程的 group 查询覆盖的 libc 静态记录。
        let status = unsafe {
            libc::getgrnam_r(
                encoded.as_ptr(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && buffer.len() < 1024 * 1024 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if status != 0 || result.is_null() {
            return Err(WorkerControlServerError::UnknownGroup);
        }
        // SAFETY: getgrnam_r 成功且 result 非空，record 已初始化；只复制不借用缓冲的 GID。
        return Ok(unsafe { record.assume_init().gr_gid });
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: geteuid 不接收指针，也不访问 Rust 管理的内存。
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
pub(super) fn validate_runtime_directory(path: &Path) -> Result<(), WorkerControlServerError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let resolved = std::fs::canonicalize(path).map_err(WorkerControlServerError::Io)?;
    if resolved != path {
        return Err(WorkerControlServerError::UnsafeRuntimeDirectory);
    }
    let owner = current_uid();
    // 仅保护叶目录不能阻止祖先被重命名；control 组只需遍历和连接权限，不拥有目录写权限。
    // 与 HTTP Unix listener 使用相同信任边界：运行账号/root 可信，root-owned sticky 祖先例外。
    for directory in path.ancestors() {
        let metadata =
            std::fs::symlink_metadata(directory).map_err(WorkerControlServerError::Io)?;
        let mode = metadata.permissions().mode();
        let protected_shared_ancestor =
            directory != path && metadata.uid() == 0 && mode & 0o1000 != 0;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || mode & 0o022 != 0 && !protected_shared_ancestor
            || metadata.uid() != owner && (directory == path || metadata.uid() != 0)
        {
            return Err(WorkerControlServerError::UnsafeRuntimeDirectory);
        }
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

    // 目录链必须先经可信权限校验；重查 inode 只拒绝已发生的替换，不宣称消除受信账号的竞态。
    if socket_identity(path)? != identity {
        return Err(WorkerControlServerError::InvalidSecurityConfig(
            "绑定后的 socket 身份已变化，拒绝修改权限",
        ));
    }
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
