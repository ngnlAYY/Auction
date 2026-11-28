//! 固定并读取备份范围目录描述符，避免符号链接与目录项替换重定向。

use std::{
    ffi::{CString, OsString},
    fs::{File, OpenOptions},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Component, Path},
};

use anyhow::Context;

use crate::support::files::{Facts, ancestor_problem, effective_uid};

pub(crate) fn open_directory(path: &Path) -> anyhow::Result<File> {
    if !path.is_absolute() {
        anyhow::bail!("备份范围目录必须是绝对路径：{}", path.display());
    }
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/")
        .context("打开文件系统根目录失败")?;
    let euid = effective_uid();
    if let Some(problem) = ancestor_problem(
        Facts::of(&directory.metadata().context("读取文件系统根目录元数据失败")?),
        euid,
    ) {
        anyhow::bail!("文件系统根目录不可信（{problem}）：{}", path.display());
    }
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            if matches!(component, Component::RootDir) {
                continue;
            }
            anyhow::bail!("备份范围目录含非普通路径组件：{}", path.display());
        };
        let name = CString::new(name.as_bytes()).context("备份范围目录名含 NUL")?;
        // SAFETY: directory is pinned and name is one NUL-free component.
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("打开备份范围目录祖先失败：{}", path.display()));
        }
        // SAFETY: openat returned a fresh descriptor owned by this call.
        let next = unsafe { File::from_raw_fd(descriptor) };
        let metadata = next.metadata().context("读取备份范围目录元数据失败")?;
        let mut entry = std::mem::MaybeUninit::<libc::stat>::zeroed();
        // SAFETY: directory and name are valid; entry points to writable stat storage.
        if unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                name.as_ptr(),
                entry.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("复核备份范围目录项失败：{}", path.display()));
        }
        // SAFETY: fstatat initialized entry after returning success.
        let entry = unsafe { entry.assume_init() };
        if entry.st_dev != metadata.dev()
            || entry.st_ino != metadata.ino()
            || entry.st_mode & libc::S_IFMT != libc::S_IFDIR
        {
            anyhow::bail!("备份范围目录项在固定时被替换：{}", path.display());
        }
        if components.peek().is_some() {
            if let Some(problem) = ancestor_problem(Facts::of(&metadata), euid) {
                anyhow::bail!("备份范围目录不可信（{problem}）：{}", path.display());
            }
        } else if metadata.mode() & 0o022 != 0 {
            anyhow::bail!("备份范围来源目录允许 group/other 写入：{}", path.display());
        }
        directory = next;
    }
    Ok(directory)
}

pub(super) fn read_directory_names(directory: &File) -> std::io::Result<Vec<OsString>> {
    // SAFETY: directory 是有效描述符；F_DUPFD_CLOEXEC 返回独立拥有的描述符，且复制出的描述符
    // 保持 close-on-exec，不会在枚举期间泄漏给并发启动的子进程。
    let descriptor = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fdopendir takes ownership of the duplicate descriptor.
    let stream = unsafe { libc::fdopendir(descriptor) };
    if stream.is_null() {
        let error = std::io::Error::last_os_error();
        // SAFETY: fdopendir failed and did not consume the descriptor.
        unsafe { libc::close(descriptor) };
        return Err(error);
    }
    let mut names = Vec::new();
    let mut failure = None;
    loop {
        // SAFETY: errno is thread-local storage for the current thread.
        unsafe { *libc::__errno_location() = 0 };
        // SAFETY: stream is a live DIR pointer owned by this function.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error().is_some_and(|code| code != 0) {
                failure = Some(error);
            }
            break;
        }
        // SAFETY: readdir returned an entry whose d_name is NUL-terminated.
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            names.push(OsString::from_vec(name.to_vec()));
        }
    }
    // SAFETY: stream is a live DIR pointer owned by this function.
    unsafe { libc::closedir(stream) };
    match failure {
        Some(error) => Err(error),
        None => Ok(names),
    }
}

pub(super) fn open_child_file(parent: &File, name: &CString) -> std::io::Result<File> {
    // SAFETY: parent is an open directory and name is a validated single component.
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: openat returned a fresh descriptor owned by this call.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}
