//! SQLite 文件库的稳定 sidecar 锁；不打开额外数据库 fd，不删除已发布锁文件。

use std::{
    fs::{self, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use super::{RealtimeConsumerOwnershipError, SqliteConsumerLock};

pub(super) struct DatabaseFile {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl DatabaseFile {
    pub(super) fn new(filename: &str) -> io::Result<Self> {
        let path = fs::canonicalize(filename)?;
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() {
            return Err(io::Error::other("SQLite main 必须是普通文件"));
        }
        Ok(Self { path, device: metadata.dev(), inode: metadata.ino() })
    }

    pub(super) fn acquire(
        &self,
        consumer_key: &str,
    ) -> Result<SqliteConsumerLock, RealtimeConsumerOwnershipError> {
        let unavailable = || RealtimeConsumerOwnershipError::Unavailable;
        let parent = self.path.parent().ok_or_else(unavailable)?;
        validate_directory_chain(parent).map_err(|_| unavailable())?;
        let metadata = fs::symlink_metadata(&self.path).map_err(|_| unavailable())?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.dev() != self.device
            || metadata.ino() != self.inode
        {
            return Err(unavailable());
        }
        // 单文件 bind mount 会让同一数据库在不同目录生成不同 sidecar，必须在取锁前拒绝。
        require_same_mount(&self.path, parent).map_err(|_| unavailable())?;
        let mut digest = Sha256::new();
        digest.update(self.path.file_name().ok_or_else(unavailable)?.as_encoded_bytes());
        digest.update([0]);
        digest.update(consumer_key.as_bytes());
        // 同目录及同文件名在 bind-mounted 目录下仍对应同一 sidecar，不受 PrivateTmp 影响。
        let lock_path = parent.join(format!(".auction-realtime-{:x}.lock", digest.finalize()));
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&lock_path)
            .map_err(|_| unavailable())?;
        let locked_metadata = lock.metadata().map_err(|_| unavailable())?;
        let path_metadata = fs::symlink_metadata(&lock_path).map_err(|_| unavailable())?;
        if !locked_metadata.is_file()
            || locked_metadata.nlink() != 1
            || locked_metadata.uid() != current_uid()
            || locked_metadata.permissions().mode() & 0o7777 != 0o600
            || !path_metadata.is_file()
            || path_metadata.dev() != locked_metadata.dev()
            || path_metadata.ino() != locked_metadata.ino()
        {
            return Err(unavailable());
        }
        // SAFETY: fd 属于本次打开且经身份验证的独立 sidecar，flock 不读写 Rust 内存。
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(if io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
                RealtimeConsumerOwnershipError::AlreadyOwned
            } else {
                unavailable()
            });
        }
        Ok(SqliteConsumerLock::File { _file: lock })
    }
}

fn current_uid() -> u32 {
    // SAFETY: geteuid 无指针参数，不读写 Rust 管理的内存。
    unsafe { libc::geteuid() }
}

#[cfg(target_os = "linux")]
fn require_same_mount(database: &Path, directory: &Path) -> io::Result<()> {
    fn mount_id(path: &Path) -> io::Result<u64> {
        let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| io::Error::other("SQLite 路径包含非法字节"))?;
        let mut metadata = std::mem::MaybeUninit::<libc::statx>::zeroed();
        // SAFETY: path 为有效 NUL 结尾路径，metadata 是独立可写 statx 缓冲区，不打开数据库 fd。
        let result = unsafe {
            libc::statx(
                libc::AT_FDCWD,
                path.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
                libc::STATX_MNT_ID,
                metadata.as_mut_ptr(),
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: statx 成功返回，结构已经初始化；仍须确认内核提供了所需 mount 身份。
        let metadata = unsafe { metadata.assume_init() };
        if metadata.stx_mask & libc::STATX_MNT_ID == 0 {
            return Err(io::Error::other("内核不能提供 SQLite mount 身份"));
        }
        Ok(metadata.stx_mnt_id)
    }
    if mount_id(database)? != mount_id(directory)? {
        return Err(io::Error::other("SQLite 消费锁不支持单文件挂载别名"));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn require_same_mount(_database: &Path, _directory: &Path) -> io::Result<()> {
    Err(io::Error::other("当前平台无法确认 SQLite mount 身份"))
}

fn validate_directory_chain(directory: &Path) -> io::Result<()> {
    let uid = current_uid();
    for ancestor in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let metadata = fs::symlink_metadata(ancestor)?;
        let mode = metadata.permissions().mode();
        let sticky_root_ancestor =
            ancestor != directory && metadata.uid() == 0 && mode & 0o1000 != 0;
        if !metadata.is_dir()
            || (metadata.uid() != 0 && metadata.uid() != uid)
            || (mode & 0o022 != 0 && !sticky_root_ancestor)
        {
            return Err(io::Error::other("SQLite 消费锁目录链不可受非信任账号替换"));
        }
    }
    Ok(())
}
