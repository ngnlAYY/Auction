//! 备份管线共用的子进程与暂存文件边界：有界 stderr 捕获、退出状态描述、临时文件清理。
//!
//! 只处理本地路径与子进程输出，不解析业务数据；失败原因只进入 stderr 日志或错误链，
//! 不写入回执文件。

use std::{
    fs, io,
    path::Path,
    process::{ExitStatus, Stdio},
    time::SystemTime,
};

use anyhow::Context;
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::app::progress;

/// 删除固定临时名的上次残留，使 `create_new` 重跑不会永久失败。
pub(crate) fn remove_stale(path: &Path, what: &str) -> anyhow::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("清理{what}失败：{}", path.display())),
    }
}

/// 尽力清理临时文件；失败只记录进度日志，不覆盖调用方正在返回的错误。
pub(crate) fn discard_stale(path: &Path, tag: &str) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => progress(format_args!("{tag} 清理临时文件失败：{error}")),
    }
}

/// 只执行 `age --version`，读取主机版本字符串；不接触私钥、口令或数据库。
pub(crate) async fn age_version() -> anyhow::Result<String> {
    let mut child = tokio::process::Command::new("age")
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

/// 有界读取子进程输出：最多保留 `limit` 字节，并报告是否溢出。
pub(crate) async fn capture(
    mut stream: impl AsyncRead + Unpin,
    limit: usize,
) -> io::Result<(Vec<u8>, bool)> {
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

/// 退出状态的简短描述；不展开平台相关细节。
pub(crate) fn describe_status(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit {code}"),
        None => "被信号终止".to_owned(),
    }
}

/// 只保留可打印文本并截断；不把无限 stderr 写进日志或回执。
pub(crate) fn describe(captured: io::Result<(Vec<u8>, bool)>) -> String {
    let (bytes, overflow) = match captured {
        Ok(value) => value,
        Err(error) => return format!("读取 stderr 失败：{error}"),
    };
    let text: String = String::from_utf8_lossy(&bytes)
        .chars()
        .filter(|character| !character.is_control())
        .take(512)
        .collect();
    let text = text.trim();
    if text.is_empty() {
        if overflow {
            "stderr 超出上限且无可用文本".to_owned()
        } else {
            "无 stderr 输出".to_owned()
        }
    } else if overflow {
        format!("{text}…")
    } else {
        text.to_owned()
    }
}

/// 当前 UNIX 秒；系统时间早于纪元时失败。
pub(crate) fn unix_seconds() -> anyhow::Result<u64> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .context("系统时间早于 UNIX 纪元")
}
