//! 暂存目录扫描与既有文件分类。

use std::fs;
use std::fs::Metadata;
use std::path::Path;

use anyhow::Context;

use super::manifest::{
    ArchiveManifest, BaseManifest, BaseReceipt, BaseStaged, Duplicate, InvalidEntry,
    SourceIdentity, Staged, StagedScan, WalUploadReceipt,
};
use super::paths::{
    base_cipher_path, base_id_from_file_name, base_manifest_path, base_receipt_path, cipher_path,
    manifest_path, valid_segment_name, wal_upload_path,
};
use super::{CIPHER_SUFFIX, MANIFEST_SUFFIX, MAX_MANIFEST_BYTES};
use crate::support::files;

/// 重复归档只认 manifest 与密文的完整证据；任何不一致都失败关闭，不覆盖。
pub(crate) fn classify(
    directory: &Path,
    segment: &str,
    source: &SourceIdentity,
) -> anyhow::Result<Duplicate> {
    let proof = wal_upload_path(directory, segment);
    if regular_or_none(&proof)?.is_some() {
        let receipt = read_wal_upload(&proof)?;
        receipt.validate(segment)?;
        if receipt.source_bytes != source.bytes || receipt.source_mtime_ns != source.mtime_ns {
            anyhow::bail!("同名 WAL 段已有不同源内容的上传证据：{}", proof.display());
        }
        return Ok(Duplicate::Uploaded(receipt));
    }
    let cipher = cipher_path(directory, segment);
    let manifest = manifest_path(directory, segment);
    let cipher_metadata = regular_or_none(&cipher)?;
    let manifest_metadata = regular_or_none(&manifest)?;
    match (cipher_metadata, manifest_metadata) {
        (None, None) => Ok(Duplicate::Absent),
        (Some(_), None) => Ok(Duplicate::UnconfirmedCipher(cipher)),
        (None, Some(_)) => {
            anyhow::bail!("WAL manifest 存在但密文缺失：{}", manifest.display())
        }
        (Some(cipher_metadata), Some(_)) => {
            let record = read_manifest(&manifest)?;
            record.validate(segment)?;
            if record.source_bytes != source.bytes || record.source_mtime_ns != source.mtime_ns {
                anyhow::bail!("同名 WAL 段已有不同源内容的归档：{}", cipher.display());
            }
            if cipher_metadata.len() != record.cipher_bytes {
                anyhow::bail!("WAL 密文长度与 manifest 不一致：{}", cipher.display());
            }
            Ok(Duplicate::Confirmed(record))
        }
    }
}

/// 枚举可上传的暂存对；格式错误逐项报告，不阻止其他项继续上传。
pub(crate) fn staged(directory: &Path) -> anyhow::Result<StagedScan> {
    let mut segments = Vec::new();
    let mut bases = Vec::new();
    for entry in fs::read_dir(directory)
        .with_context(|| format!("枚举暂存目录失败：{}", directory.display()))?
    {
        let entry =
            entry.with_context(|| format!("读取暂存目录项失败：{}", directory.display()))?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else { continue };
        // 基础备份名与 WAL 段名互不重叠：先识别基础备份，再按 WAL 后缀归类。
        if let Some(backup_id) = base_id_from_file_name(file_name) {
            bases.push(backup_id.to_owned());
            continue;
        }
        // 密文与 manifest 都登记段名：只剩一半的暂存项也进入 incomplete，避免静默遗留。
        let segment = file_name
            .strip_suffix(CIPHER_SUFFIX)
            .or_else(|| file_name.strip_suffix(MANIFEST_SUFFIX));
        let Some(segment) = segment else { continue };
        if valid_segment_name(segment) {
            segments.push(segment.to_owned());
        }
    }
    segments.sort();
    segments.dedup();
    bases.sort();
    bases.dedup();

    let mut scan = StagedScan {
        ready: Vec::new(),
        base: Vec::new(),
        incomplete: Vec::new(),
        invalid: Vec::new(),
    };
    for segment in segments {
        let cipher = cipher_path(directory, &segment);
        let manifest = manifest_path(directory, &segment);
        let Some(cipher_metadata) = regular_or_none(&cipher)? else {
            // 只剩 manifest（或枚举后消失）：上传器无法重建密文，只报告不删除。
            scan.incomplete.push(segment);
            continue;
        };
        if regular_or_none(&manifest)?.is_none() {
            // 归档进程先发布密文、再发布 manifest；缺 manifest 表示本次尚未确认。
            scan.incomplete.push(segment);
            continue;
        }
        let record = match read_manifest(&manifest).and_then(|record| {
            record.validate(&segment)?;
            Ok(record)
        }) {
            Ok(record) => record,
            Err(error) => {
                scan.invalid.push(InvalidEntry { name: segment, reason: format!("{error:#}") });
                continue;
            }
        };
        if cipher_metadata.len() != record.cipher_bytes {
            scan.invalid.push(InvalidEntry {
                name: segment,
                reason: "WAL 密文长度与 manifest 不一致".to_owned(),
            });
            continue;
        }
        scan.ready.push(Staged { segment_name: segment, cipher, manifest, record });
    }
    for backup_id in bases {
        let cipher = base_cipher_path(directory, &backup_id);
        let manifest = base_manifest_path(directory, &backup_id);
        let Some(cipher_metadata) = regular_or_none(&cipher)? else {
            // 基础备份创建器先发布密文、再发布 manifest；缺一半时不删除，只报告。
            scan.incomplete.push(backup_id);
            continue;
        };
        if regular_or_none(&manifest)?.is_none() {
            scan.incomplete.push(backup_id);
            continue;
        }
        let record = match read_base_manifest(&manifest).and_then(|record| {
            record.validate(&backup_id)?;
            Ok(record)
        }) {
            Ok(record) => record,
            Err(error) => {
                scan.invalid.push(InvalidEntry { name: backup_id, reason: format!("{error:#}") });
                continue;
            }
        };
        if cipher_metadata.len() != record.cipher_bytes {
            scan.invalid.push(InvalidEntry {
                name: backup_id,
                reason: "基础备份密文长度与 manifest 不一致".to_owned(),
            });
            continue;
        }
        let receipt = read_base_receipt(directory, &backup_id)?;
        let receipt_destination = receipt.as_ref().map(|receipt| receipt.destination.clone());
        scan.base.push(BaseStaged {
            backup_id,
            cipher,
            manifest,
            record,
            uploaded: receipt.is_some(),
            receipt_destination,
        });
    }
    Ok(scan)
}

/// 有界读取基础备份 manifest；拒绝符号链接、特殊文件与超出上限的输入。
pub(super) fn read_base_manifest(path: &Path) -> anyhow::Result<BaseManifest> {
    let bytes = files::read_bounded(path, MAX_MANIFEST_BYTES)?;
    serde_json::from_slice(&bytes).context("解析基础备份 manifest 失败")
}

/// 本机上传证据无效或缺失时视为尚未上传：宁可多一次完整回读，也不能跳过真正未上传的备份。
pub(super) fn read_base_receipt(
    directory: &Path,
    backup_id: &str,
) -> anyhow::Result<Option<BaseReceipt>> {
    let path = base_receipt_path(directory, backup_id);
    let Some(metadata) = regular_or_none(&path)? else { return Ok(None) };
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Ok(None);
    }
    let bytes = files::read_bounded(&path, MAX_MANIFEST_BYTES)?;
    let Ok(receipt) = serde_json::from_slice::<BaseReceipt>(&bytes) else { return Ok(None) };
    Ok(receipt.validate(backup_id).ok().map(|()| receipt))
}

/// 只接受 64 位小写十六进制摘要。
pub(super) fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

pub(super) fn read_wal_upload(path: &Path) -> anyhow::Result<WalUploadReceipt> {
    let bytes = files::read_bounded(path, MAX_MANIFEST_BYTES)?;
    serde_json::from_slice(&bytes).context("解析 WAL 上传证据失败")
}

/// 有界读取 manifest；拒绝符号链接、特殊文件与超出上限的输入。
pub(super) fn read_manifest(path: &Path) -> anyhow::Result<ArchiveManifest> {
    let bytes = files::read_bounded(path, MAX_MANIFEST_BYTES)?;
    serde_json::from_slice(&bytes).context("解析 WAL 归档 manifest 失败")
}

/// 只接受普通文件；缺失返回 None，符号链接与特殊文件直接失败。
pub(super) fn regular_or_none(path: &Path) -> anyhow::Result<Option<Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            anyhow::bail!("暂存项必须是普通文件：{}", path.display())
        }
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("读取暂存项失败：{}", path.display())),
    }
}
