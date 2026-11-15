//! 一次性证据与回执文件：父目录私有、目标不存在才以 create_new/0600 写入，并同步父目录。

use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

use anyhow::Context;

use super::{effective_uid, sync_directory};

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
        Err(error) => Err(error).with_context(|| format!("检查输出文件失败：{}", path.display())),
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
    sync_directory(parent, "目录")
}
