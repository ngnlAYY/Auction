//! 备份范围的 age 文件级加解密；只处理主机路径，不读取或记录明文内容。

use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    process::Stdio,
};

use std::fs::File;

use anyhow::Context;
use tokio::process::Command;

use crate::support::{files, process};

/// 用 age 公钥加密一个已封闭的明文文件，随后以硬链接无覆盖发布并同步父目录。
pub(crate) async fn seal_file(input: &Path, output: &Path, recipient: &str) -> anyhow::Result<()> {
    files::require_private_target(output)?;
    let temporary = partial(output);
    process::remove_stale(&temporary, "残留临时文件")?;
    let mut child = Command::new("age")
        .args(["--encrypt", "--recipient", recipient, "--output"])
        .arg(&temporary)
        .arg(input)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("age 加密无法启动")?;
    let stderr = child.stderr.take().context("age stderr 不可用")?;
    let (status, _) = tokio::try_join!(child.wait(), process::capture(stderr, 16 * 1024))?;
    if !status.success() {
        let _ = fs::remove_file(&temporary);
        anyhow::bail!("age 加密失败");
    }
    publish(&temporary, output)
}

/// 用离线 age 私钥解密到输出目录内的私有临时文件；私钥只作参数，不复制或读入内存。
pub(crate) async fn decrypt_file(
    identity: &Path,
    input: &Path,
    output: &Path,
) -> anyhow::Result<()> {
    require_identity(identity)?;
    files::require_private_target(output)?;
    let temporary = partial(output);
    process::remove_stale(&temporary, "残留临时文件")?;
    let mut child = Command::new("age")
        .args(["--decrypt", "--identity"])
        .arg(identity)
        .arg("--output")
        .arg(&temporary)
        .arg(input)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("age 解密无法启动")?;
    let stderr = child.stderr.take().context("age stderr 不可用")?;
    let (status, _) = tokio::try_join!(child.wait(), process::capture(stderr, 16 * 1024))?;
    if !status.success() {
        let _ = fs::remove_file(&temporary);
        anyhow::bail!("age 解密失败；请核对离线私钥与备份收件人");
    }
    publish(&temporary, output)
}

fn require_identity(identity: &Path) -> anyhow::Result<()> {
    if !identity.is_absolute() {
        anyhow::bail!("age 私钥必须使用绝对路径");
    }
    let file = files::open_regular(identity)?;
    let metadata = file.metadata().context("读取 age 私钥元数据失败")?;
    if metadata.uid() != files::effective_uid()
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
        anyhow::bail!("age 私钥必须是当前账号拥有的 owner-only 单链接普通文件");
    }
    Ok(())
}

fn partial(output: &Path) -> std::path::PathBuf {
    let name = output.file_name().and_then(|name| name.to_str()).unwrap_or("scope");
    output.with_file_name(format!(".{name}.partial"))
}

/// 硬链接发布不会覆盖同名文件；临时名只由本次调用使用，失败时删掉。
fn publish(temporary: &Path, output: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(temporary)
        .with_context(|| format!("读取临时密文失败：{}", temporary.display()))?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.uid() != files::effective_uid() {
        let _ = fs::remove_file(temporary);
        anyhow::bail!("age 输出必须是当前账号拥有的单链接普通文件");
    }
    // age 创建输出时可能继承更宽的 umask（例如测试进程）；发布前显式收紧到 0600。
    fs::set_permissions(temporary, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("收紧临时密文权限失败：{}", temporary.display()))?;
    let metadata = fs::symlink_metadata(temporary)
        .with_context(|| format!("读取临时密文失败：{}", temporary.display()))?;
    if metadata.len() == 0 || metadata.mode() & 0o077 != 0 {
        let _ = fs::remove_file(temporary);
        anyhow::bail!("age 输出不是非空的 owner-only 普通文件");
    }
    let result = fs::hard_link(temporary, output)
        .with_context(|| format!("发布加密文件失败：{}", output.display()));
    let _ = fs::remove_file(temporary);
    result?;
    files::sync_parent(output)
}

/// 摘要读取只接受当前账号的 owner-only 单链接普通文件。
pub(crate) fn open_plain(path: &Path) -> anyhow::Result<File> {
    let file = files::open_regular(path)?;
    let metadata = file.metadata().context("读取范围文件元数据失败")?;
    if metadata.mode() & 0o077 != 0 || metadata.nlink() != 1 {
        anyhow::bail!("范围文件必须是 owner-only 单链接普通文件：{}", path.display());
    }
    Ok(file)
}
