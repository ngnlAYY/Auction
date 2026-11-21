//! 通过目录 inode 上的跨进程锁串行化目录项变更。

use std::{
    fs::File,
    io::ErrorKind,
    os::fd::{AsRawFd, FromRawFd},
};

use anyhow::Context;

/// owner-only 权限隔离其他 UID；同 UID 调用由 inode 锁串行化。
pub(super) fn lock_directory_entries(directory: &File) -> anyhow::Result<File> {
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            c".".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error()).context("固定私有目录变更锁失败");
    }
    // SAFETY：openat 成功，descriptor 由当前调用独占拥有。
    let lock = unsafe { File::from_raw_fd(descriptor) };
    loop {
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(lock);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != ErrorKind::Interrupted {
            return Err(error).context("取得私有目录变更锁失败");
        }
    }
}
