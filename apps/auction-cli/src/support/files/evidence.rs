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
    require_private_directory(parent, "输出父目录")?;
    match fs::symlink_metadata(path) {
        Ok(_) => anyhow::bail!("输出文件已存在，拒绝覆盖：{}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("检查输出文件失败：{}", path.display())),
    }
}

/// 一次性证据的目录口径：已存在、非符号链接、禁止 group/other 写入，且归当前账号或 root 所有
/// （root 运行时只能是 root）。只读，不创建目录；deploy 停机前校验 `--backup-dir` 与停机后写入
/// 停机备份共用这一口径，`what` 只用于错误文案。
pub(crate) fn require_private_directory(directory: &Path, what: &str) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(directory)
        .with_context(|| format!("读取{what}失败：{}", directory.display()))?;
    let real_directory = metadata.is_dir() && !metadata.file_type().is_symlink();
    match directory_problem(real_directory, metadata.uid(), metadata.mode(), effective_uid()) {
        Some(problem) => anyhow::bail!("{what}{problem}：{}", directory.display()),
        None => Ok(()),
    }
}

/// [`require_private_directory`] 的纯判定，便于在非 root 测试中覆盖 root 口径。
fn directory_problem(real_directory: bool, uid: u32, mode: u32, euid: u32) -> Option<&'static str> {
    if !real_directory || mode & 0o022 != 0 {
        return Some("必须是非符号链接目录且禁止 group/other 写入");
    }
    if uid != euid && uid != 0 {
        return Some("必须归当前账号或 root 所有");
    }
    None
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

#[cfg(test)]
mod tests {
    use super::directory_problem;

    const SERVICE_UID: u32 = 990;

    /// root 运行时只接受 root 所有的目录：服务账号所有的 0700 目录拒绝（否则要到停机后写停机
    /// 备份时才失败）；group/other 可写与非真实目录同样拒绝。非 root 运行接受自己或 root 的目录。
    #[test]
    fn private_directory_requires_a_trusted_owner() {
        assert_eq!(directory_problem(true, 0, 0o40_700, 0), None);
        assert!(directory_problem(true, SERVICE_UID, 0o40_700, 0).is_some());
        assert!(directory_problem(true, 0, 0o40_770, 0).is_some());
        assert!(directory_problem(true, 0, 0o40_702, 0).is_some());
        assert!(directory_problem(false, 0, 0o40_700, 0).is_some());
        assert_eq!(directory_problem(true, 1000, 0o40_700, 1000), None);
        assert_eq!(directory_problem(true, 0, 0o40_755, 1000), None);
        assert!(directory_problem(true, SERVICE_UID, 0o40_700, 1000).is_some());
    }
}
