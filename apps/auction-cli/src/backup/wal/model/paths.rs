//! 暂存文件命名、路径与目录校验。

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Context;

use super::manifest::{BaseStaged, Retention, SourceIdentity};
use super::{
    BASE_CIPHER_SUFFIX, BASE_ID_PREFIX, BASE_MANIFEST_SUFFIX, BASE_RECEIPT_SUFFIX, BECH32,
    CIPHER_SUFFIX, MANIFEST_SUFFIX, WAL_UPLOAD_SUFFIX,
};

/// PostgreSQL 的 `%f` 只可能是 WAL 段、时间线历史文件或基础备份标签。
pub(crate) fn valid_segment_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    match bytes.len() {
        24 => bytes.iter().all(u8::is_ascii_hexdigit),
        16 => name.ends_with(".history") && bytes[..8].iter().all(u8::is_ascii_hexdigit),
        40 => {
            name.ends_with(".backup")
                && bytes[24] == b'.'
                && bytes[25..33].iter().all(u8::is_ascii_hexdigit)
                && bytes[..24].iter().all(u8::is_ascii_hexdigit)
        }
        _ => false,
    }
}

/// 与配置校验同一形状：62 字节、`age1` 前缀、bech32 字符集；私钥不进入服务器。
pub(crate) fn valid_recipient(recipient: &str) -> bool {
    let bytes = recipient.as_bytes();
    bytes.len() == 62
        && recipient.starts_with("age1")
        && bytes[4..].iter().all(|byte| BECH32.contains(byte))
}

pub(crate) fn cipher_path(directory: &Path, segment: &str) -> PathBuf {
    directory.join(format!("{segment}{CIPHER_SUFFIX}"))
}

pub(crate) fn manifest_path(directory: &Path, segment: &str) -> PathBuf {
    directory.join(format!("{segment}{MANIFEST_SUFFIX}"))
}

pub(crate) fn wal_upload_path(directory: &Path, segment: &str) -> PathBuf {
    directory.join(format!("{segment}{WAL_UPLOAD_SUFFIX}"))
}

/// 基础备份 ID 固定为 `base-YYYYMMDDTHHMMSSZ`；只从 UTC 时刻派生，不含路径分隔符。
pub(crate) fn valid_base_id(backup_id: &str) -> bool {
    let Some(stamp) = backup_id.strip_prefix(BASE_ID_PREFIX) else { return false };
    let bytes = stamp.as_bytes();
    bytes.len() == 16
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[8] == b'T'
        && bytes[9..15].iter().all(u8::is_ascii_digit)
        && bytes[15] == b'Z'
}

/// 由当前 UTC 时刻派生备份 ID；同一秒内的重复运行得到同一身份。
pub(crate) fn base_id(now: chrono::DateTime<chrono::Utc>) -> String {
    format!("{BASE_ID_PREFIX}{}", now.format("%Y%m%dT%H%M%SZ"))
}

pub(crate) fn base_cipher_path(directory: &Path, backup_id: &str) -> PathBuf {
    directory.join(format!("{backup_id}{BASE_CIPHER_SUFFIX}"))
}

pub(crate) fn base_manifest_path(directory: &Path, backup_id: &str) -> PathBuf {
    directory.join(format!("{backup_id}{BASE_MANIFEST_SUFFIX}"))
}

pub(crate) fn base_receipt_path(directory: &Path, backup_id: &str) -> PathBuf {
    directory.join(format!("{backup_id}{BASE_RECEIPT_SUFFIX}"))
}

/// 文件名对应的基础备份身份；不是基础备份名时返回 None。
pub(crate) fn base_id_from_file_name(name: &str) -> Option<&str> {
    let backup_id = name
        .strip_suffix(BASE_CIPHER_SUFFIX)
        .or_else(|| name.strip_suffix(BASE_MANIFEST_SUFFIX))
        .or_else(|| name.strip_suffix(BASE_RECEIPT_SUFFIX))?;
    valid_base_id(backup_id).then_some(backup_id)
}

/// 只保留最新 `keep` 份完整基础备份；更旧的完整暂存项可由上传器删除。
///
/// 只考虑密文与 manifest 都通过长度校验的项，`extend_keep` 中的 ID（本次上传失败的项）
/// 也保留，避免上传中断后先删掉唯一的本地副本。
pub(crate) fn retention_plan(
    mut complete: Vec<BaseStaged>,
    keep: usize,
    extend_keep: &[String],
) -> Retention {
    complete.sort_by(|left, right| right.backup_id.cmp(&left.backup_id));
    let mut retained = Vec::new();
    let mut removable = Vec::new();
    for (index, set) in complete.into_iter().enumerate() {
        if index < keep || extend_keep.contains(&set.backup_id) {
            retained.push(set.backup_id);
        } else {
            removable.push(set);
        }
    }
    Retention { retained, removable }
}

/// 暂存目录必须已存在：归档/基础备份账号与上传账号都需要写入/删除权限，因此不创建也不改权限。
pub(crate) fn require_staging_dir(directory: &Path) -> anyhow::Result<()> {
    if !directory.is_absolute() {
        anyhow::bail!("暂存目录必须是绝对路径：{}", directory.display());
    }
    let metadata = fs::symlink_metadata(directory)
        .with_context(|| format!("读取暂存目录失败：{}", directory.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        anyhow::bail!("暂存目录必须是非符号链接目录：{}", directory.display());
    }
    if metadata.mode() & 0o002 != 0 {
        anyhow::bail!("暂存目录禁止 other 写入：{}", directory.display());
    }
    Ok(())
}

/// 源文件必须是普通文件；只接受 PostgreSQL 传入的 WAL 文件本身。
pub(crate) fn source_identity(path: &Path) -> anyhow::Result<SourceIdentity> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("读取 WAL 文件失败：{}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        anyhow::bail!("WAL 源必须是普通文件：{}", path.display());
    }
    let modified = metadata.modified().context("读取 WAL 文件时间失败")?;
    let elapsed =
        modified.duration_since(SystemTime::UNIX_EPOCH).context("WAL 文件时间早于 UNIX 纪元")?;
    let mtime_ns = u64::try_from(elapsed.as_nanos()).context("WAL 文件时间超出可记录范围")?;
    Ok(SourceIdentity { bytes: metadata.len(), mtime_ns })
}
