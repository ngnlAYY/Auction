//! 主机 age 进程的公钥加密；有界消费 stderr，密文封存后永不重新加密。

use std::{path::Path, process::Stdio};

use anyhow::Context;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

use super::super::filesystem::{FileIdentity, PrivateDirectory};
use super::{model::Cipher, state::missing};
use crate::backup::manifest::sha256_file;

/// 预检只执行版本查询，不读取数据库、私钥或口令。
pub(super) async fn preflight() -> anyhow::Result<String> {
    let mut child = Command::new("age")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("age 不可用")?;
    let stdout = child.stdout.take().context("age stdout 不可用")?;
    let stderr = child.stderr.take().context("age stderr 不可用")?;
    let (status, (bytes, overflow), _) =
        tokio::try_join!(child.wait(), capture(stdout, 256), capture(stderr, 16 * 1024))?;
    let version = std::str::from_utf8(&bytes).context("age 版本格式无效")?.trim();
    if !status.success()
        || overflow
        || version.is_empty()
        || !version.bytes().all(|byte| byte.is_ascii_alphanumeric() || b" ._+-()".contains(&byte))
    {
        anyhow::bail!("age 版本预检失败");
    }
    Ok(version.to_owned())
}

/// 无记录而有已发布密文表示上次在封存后、checkpoint 前中断；认领原字节，不能重新加密。
pub(super) async fn seal(
    directory: &PrivateDirectory,
    input: &Path,
    plain_identity: FileIdentity,
    file_name: &str,
    recipient: &str,
    existing: Option<&Cipher>,
) -> anyhow::Result<Cipher> {
    if let Some(existing) = existing {
        validate(directory, existing).await?;
        if existing.file_name != file_name {
            anyhow::bail!("密文名称不一致");
        }
        return Ok(existing.clone());
    }
    match directory.open(file_name) {
        Ok(_) => return inspect(directory, file_name).await,
        Err(error) if missing(&error) => {}
        Err(error) => return Err(error),
    }
    let temporary = format!("{file_name}.partial");
    // 未封存临时文件从未允许上传；持锁后可以丢弃它并重做，已封存文件绝不重做。
    match directory.open(&temporary) {
        Ok(file) => directory.remove(&temporary, FileIdentity::of(&file.metadata()?))?,
        Err(error) if missing(&error) => {}
        Err(error) => return Err(error),
    }
    // main 在启动时设置的 umask 077 会由 age 继承；让 age 自行创建私有临时文件，
    // 不依赖其覆盖已存在输出文件的行为。封存前仍通过目录句柄检查 owner/权限/单链接。
    let input_file = crate::support::files::open_regular(input)?;
    if FileIdentity::of(&input_file.metadata()?) != plain_identity {
        anyhow::bail!("明文备份身份变化");
    }
    let mut child = Command::new("age")
        .args(["--encrypt", "--recipient", recipient, "--output"])
        .arg(directory.path.join(&temporary))
        .arg(input)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("age 加密无法启动")?;
    let stderr = child.stderr.take().context("age stderr 不可用")?;
    let (status, _) = tokio::try_join!(child.wait(), capture(stderr, 16 * 1024))?;
    if !status.success() {
        anyhow::bail!("age 加密失败");
    }
    let identity = FileIdentity::of(&directory.open(&temporary)?.metadata()?);
    if identity.bytes == 0
        || FileIdentity::of(&crate::support::files::open_regular(input)?.metadata()?)
            != plain_identity
    {
        anyhow::bail!("age 加密期间文件身份变化");
    }
    directory.publish_file(&temporary, file_name)?;
    inspect(directory, file_name).await
}

pub(super) async fn validate(directory: &PrivateDirectory, cipher: &Cipher) -> anyhow::Result<()> {
    let actual = inspect(directory, &cipher.file_name).await?;
    if actual.identity != cipher.identity || actual.sha256 != cipher.sha256 {
        anyhow::bail!("暂存密文身份或摘要变化，禁止重加密");
    }
    Ok(())
}

async fn inspect(directory: &PrivateDirectory, name: &str) -> anyhow::Result<Cipher> {
    let identity = FileIdentity::of(&directory.open(name)?.metadata()?);
    let sha256 = sha256_file(&directory.path.join(name)).await?;
    if identity.bytes == 0 || FileIdentity::of(&directory.open(name)?.metadata()?) != identity {
        anyhow::bail!("密文读取期间变化");
    }
    Ok(Cipher { file_name: name.to_owned(), identity, sha256 })
}

async fn capture(
    mut stream: impl AsyncRead + Unpin,
    limit: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut kept = Vec::new();
    let mut overflow = false;
    let mut buffer = [0_u8; 4096];
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let retain = count.min(limit.saturating_sub(kept.len()));
        kept.extend_from_slice(&buffer[..retain]);
        overflow |= retain != count;
    }
    Ok((kept, overflow))
}
