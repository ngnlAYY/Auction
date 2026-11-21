//! 私有文件、锁文件与原子发布的唯一边界：拒绝符号链接与特殊文件，新文件只以 create_new
//! 和 0600 发布，并在报告成功前同步文件与父目录。`directory` 提供以目录描述符固定的私有目录
//! （无覆盖或可替换发布、按身份删除、相对描述符派生子目录），`disk` 读取文件系统容量，
//! `evidence` 写一次性证据文件，`trust` 是祖先目录信任的纯判定。

use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::Path,
};

use anyhow::Context;

mod directory;
mod disk;
mod evidence;
mod trust;

pub(crate) use directory::{FileIdentity, MAX_READ_BYTES, PrivateDirectory};
pub(crate) use disk::{BTRFS_FILESYSTEM_TYPE, DiskUsage, disk_usage};
pub(crate) use evidence::{
    require_private_directory, require_private_target, sync_parent, write_private,
};
pub(crate) use trust::{Facts, Kind, ancestor_problem};

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

/// 打开归当前账号所有、单链接且权限严格为 0600 的普通文件，并基于同一描述符核验。
pub(crate) fn open_private_regular(path: &Path) -> anyhow::Result<File> {
    let file = open_regular(path)?;
    let metadata = file.metadata().context("读取私有文件元数据失败")?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o7777 != 0o600
    {
        anyhow::bail!("私有文件必须归当前账号所有、单链接且权限为 0600：{}", path.display());
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
