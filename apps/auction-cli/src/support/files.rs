//! 锁文件、私有证据与 manifest 的文件边界：拒绝符号链接与特殊文件，新文件只以 create_new
//! 和 0600 发布，并在报告成功前同步文件与父目录。

use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::Path,
};

use anyhow::Context;

/// 当前进程的有效 UID。
pub(crate) fn effective_uid() -> u32 {
    // SAFETY：geteuid 不接收指针，只读取当前进程凭据。
    unsafe { libc::geteuid() }
}

/// 创建或复核 owner-only 私有目录；已存在时必须是当前账号拥有、无 group/other 权限的目录。
pub(crate) fn ensure_private_dir(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute() {
        anyhow::bail!("私有目录必须是绝对路径：{}", path.display());
    }
    match fs::DirBuilder::new().recursive(true).mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error).with_context(|| format!("创建私有目录失败：{}", path.display()));
        }
    }
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("读取私有目录失败：{}", path.display()))?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o077 != 0
    {
        anyhow::bail!("私有目录必须归当前账号所有且禁止 group/other 访问：{}", path.display());
    }
    Ok(())
}

/// 以非阻塞方式取得进程级独占锁文件；文件描述符关闭即释放锁。
pub(crate) fn lock_file(path: &Path, busy: &str) -> anyhow::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .with_context(|| format!("打开锁文件失败：{}", path.display()))?;
    verify_owner_only(&file, path)?;
    try_lock(&file, busy)?;
    Ok(file)
}

/// 打开已存在的锁文件并取得独占锁；文件缺失时返回 None，不创建新锁。
pub(crate) fn lock_existing(path: &Path, busy: &str) -> anyhow::Result<Option<File>> {
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("打开锁文件失败：{}", path.display()));
        }
    };
    verify_owner_only(&file, path)?;
    try_lock(&file, busy)?;
    Ok(Some(file))
}

fn verify_owner_only(file: &File, path: &Path) -> anyhow::Result<()> {
    let metadata = file.metadata().context("读取锁文件元数据失败")?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o077 != 0
    {
        anyhow::bail!("锁文件必须是当前账号拥有的单链接 owner-only 普通文件：{}", path.display());
    }
    Ok(())
}

fn try_lock(file: &File, busy: &str) -> anyhow::Result<()> {
    match file.try_lock() {
        Ok(()) => Ok(()),
        Err(fs::TryLockError::WouldBlock) => anyhow::bail!("{busy}"),
        Err(fs::TryLockError::Error(error)) => Err(error).context("获取文件锁失败"),
    }
}

/// 同步目录，使新建、改名或删除的目录项在崩溃后可见；`what` 只用于错误文案。
#[cfg(feature = "postgres")]
pub(crate) fn sync_directory(directory: &Path, what: &str) -> anyhow::Result<()> {
    File::open(directory)
        .and_then(|handle| handle.sync_all())
        .with_context(|| format!("同步{what}失败：{}", directory.display()))
}

/// 以不跟随符号链接、非阻塞方式打开普通文件，类型检查与后续读取使用同一描述符。
pub(crate) fn open_regular(path: &Path) -> anyhow::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("打开文件失败：{}", path.display()))?;
    if !file.metadata().context("读取文件元数据失败")?.is_file() {
        anyhow::bail!("必须是普通文件：{}", path.display());
    }
    Ok(file)
}

/// 限量读取；文件增长超过上限同样失败，不依赖读取前的长度。
pub(crate) fn read_bounded(path: &Path, limit: u64) -> anyhow::Result<Vec<u8>> {
    let file = open_regular(path)?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("读取文件失败：{}", path.display()))?;
    if bytes.len() as u64 > limit {
        anyhow::bail!("文件超过 {limit} 字节上限：{}", path.display());
    }
    Ok(bytes)
}

#[cfg(feature = "postgres")]
pub(crate) use evidence::*;

#[cfg(feature = "postgres")]
mod evidence {
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
        path::Path,
    };

    use anyhow::Context;

    use super::effective_uid;

    /// 证据与回执的父目录必须已存在、为非符号链接目录，且不允许 group/other 写入。
    pub(crate) fn require_private_target(path: &Path) -> anyhow::Result<()> {
        if !path.is_absolute() || path.file_name().is_none() {
            anyhow::bail!("输出路径必须是带文件名的绝对路径：{}", path.display());
        }
        let parent = path.parent().context("输出路径缺少父目录")?;
        let metadata = fs::symlink_metadata(parent)
            .with_context(|| format!("读取输出父目录失败：{}", parent.display()))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() || metadata.mode() & 0o022 != 0 {
            anyhow::bail!(
                "输出父目录必须是非符号链接目录且禁止 group/other 写入：{}",
                parent.display()
            );
        }
        if metadata.uid() != effective_uid() && metadata.uid() != 0 {
            anyhow::bail!("输出父目录必须归当前账号或 root 所有：{}", parent.display());
        }
        match fs::symlink_metadata(path) {
            Ok(_) => anyhow::bail!("输出文件已存在，拒绝覆盖：{}", path.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("检查输出文件失败：{}", path.display()))
            }
        }
    }

    /// 以 create_new/0600 写入新文件，完成文件与父目录 fsync 后才返回成功。
    pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        require_private_target(path)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .with_context(|| format!("创建私有文件失败：{}", path.display()))?;
        file.write_all(bytes).context("写入私有文件失败")?;
        file.sync_all().context("同步私有文件失败")?;
        sync_parent(path)
    }

    /// 同步父目录，使新建、改名或删除的目录项在崩溃后可见。
    pub(crate) fn sync_parent(path: &Path) -> anyhow::Result<()> {
        let parent = path.parent().context("路径缺少父目录")?;
        super::sync_directory(parent, "目录")
    }
}
