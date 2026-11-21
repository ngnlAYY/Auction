//! 私有目录路径与条目名称的安全校验。

use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::ErrorKind,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Component, Path, PathBuf},
};

use anyhow::Context;

use super::super::{
    effective_uid,
    trust::{Facts, Kind, ancestor_problem, first_untrusted},
};
pub(super) fn open_trusted_ancestors(path: &Path) -> anyhow::Result<File> {
    if !path.is_absolute()
        || path.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        anyhow::bail!("私有目录父路径必须为不含 . 或 .. 的绝对路径：{}", path.display());
    }
    let euid = effective_uid();
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/")
        .context("打开根目录以固定私有目录祖先失败")?;
    let root_facts = Facts::of(&directory.metadata().context("读取根目录元数据失败")?);
    if let Some(problem) = ancestor_problem(root_facts, euid) {
        anyhow::bail!("私有目录根祖先不可信（{problem}）：{}", path.display());
    }
    let mut prefix = PathBuf::from("/");
    for part in path.components() {
        let Component::Normal(component) = part else {
            if matches!(part, Component::RootDir) {
                continue;
            }
            anyhow::bail!("私有目录父路径包含非普通项：{}", path.display());
        };
        prefix.push(component);
        let name = CString::new(component.as_bytes()).context("私有目录祖先名称包含 NUL")?;
        // SAFETY：父目录描述符固定，name 是一个普通路径分量。
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("打开私有目录祖先失败：{}", prefix.display()));
        }
        // SAFETY：openat 成功，descriptor 由当前调用独占拥有。
        let next = unsafe { File::from_raw_fd(descriptor) };
        let metadata = next
            .metadata()
            .with_context(|| format!("读取私有目录祖先失败：{}", prefix.display()))?;
        if let Some(problem) = ancestor_problem(Facts::of(&metadata), euid) {
            anyhow::bail!("私有目录祖先不可信（{problem}）：{}", prefix.display());
        }
        directory = next;
    }
    Ok(directory)
}

pub(super) fn restore_quarantined_directory(
    directory: std::os::fd::RawFd,
    quarantine: &CString,
    original: &CString,
    display_name: &str,
) -> anyhow::Result<()> {
    // SAFETY：目录描述符保持固定，两个名称均为已验证的单路径分量。
    if unsafe {
        libc::renameat2(
            directory,
            quarantine.as_ptr(),
            directory,
            original.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error())
            .context(format!("无法恢复隔离的私有子目录 {display_name}"));
    }
    Ok(())
}

pub(super) fn name_component(name: &str) -> anyhow::Result<CString> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        anyhow::bail!("私有目录项必须是无控制字符的单个文件名");
    }
    CString::new(name).context("私有目录项名称无效")
}

pub(super) fn require_private_file(file: &File) -> anyhow::Result<()> {
    let metadata = file.metadata().context("读取私有文件元数据失败")?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o077 != 0
    {
        anyhow::bail!("私有文件必须是当前账号所有的单链接 owner-only 普通文件");
    }
    Ok(())
}
/// 路径须为不含 `.`/`..` 的绝对路径；从根开始已存在的每一级（含目录本身）都须是归 root 或
/// 当前账号、禁止 group/other 写入的真实目录（root 的 sticky 目录除外，判定见
/// [`first_untrusted`]），到第一个不存在的层级为止。
pub(super) fn require_safe_path(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute()
        || path.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        anyhow::bail!("私有目录必须为不含 . 或 .. 的绝对路径：{}", path.display());
    }
    let mut prefix = PathBuf::new();
    let mut levels = Vec::new();
    for part in path.components() {
        prefix.push(part.as_os_str());
        let metadata = match fs::symlink_metadata(&prefix) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => break,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("读取私有目录祖先失败：{}", prefix.display()));
            }
        };
        let facts = Facts::of(&metadata);
        levels.push((prefix.clone(), facts));
        // 非目录（含符号链接）无法作为祖先，判定在这一级结束，不再穿过它读取下一级。
        if facts.kind != Kind::Directory {
            break;
        }
    }
    let chain = levels.iter().map(|(_, facts)| *facts).collect::<Vec<_>>();
    if let Some((index, problem)) = first_untrusted(&chain, effective_uid()) {
        let prefix = levels.get(index).map_or(path, |(prefix, _)| prefix.as_path());
        anyhow::bail!(
            "私有目录的祖先必须是 root 或当前账号所有、禁止他人写入的真实目录：{}（{problem}）",
            prefix.display()
        );
    }
    Ok(())
}
