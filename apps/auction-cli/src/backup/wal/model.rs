//! WAL 暂存命名、归档 manifest、每周基础备份 manifest 与重复判定；只描述本地证据，
//! 不访问数据库或 R2。
//!
//! 暂存目录由 PostgreSQL 服务账号写入、由上传账号读取并删除，因此不能用
//! `PrivateDirectory`（它要求执行账号独占 0700）；这里只要求目录非符号链接且
//! 禁止 other 写入，具体 owner/group 由部署方按 RECOVERY.md 设置。

use std::{
    fs::{self, Metadata},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::support::files;

/// 归档 manifest 的格式名；字段只追加，旧文件继续可读。
pub(super) const MANIFEST_FORMAT: &str = "auction-wal-archive.v1";
/// 密文文件名固定为 `<段名>.zst.age`。
pub(super) const CIPHER_SUFFIX: &str = ".zst.age";
/// manifest 文件名固定为 `<段名>.zst.age.manifest.json`。
pub(super) const MANIFEST_SUFFIX: &str = ".zst.age.manifest.json";
/// manifest 只是本机证据，不承载 WAL 内容；上限与备份 manifest 一致。
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// 上传完成后保留的持久证据；密文与 manifest 删除后仍能让 PostgreSQL 重试识别已归档。
pub(super) const WAL_UPLOAD_SUFFIX: &str = ".wal-upload.json";
/// WAL 上传证据格式名。
pub(super) const WAL_UPLOAD_FORMAT: &str = "auction-wal-upload.v1";
/// age X25519 收件人公钥的 bech32 数据字符集。
const BECH32: &[u8] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

/// 基础备份身份前缀；文件名与对象键都由它派生，不与 24 位 WAL 段名重叠。
pub(super) const BASE_ID_PREFIX: &str = "base-";
/// 基础备份密文文件名固定为 `<备份 ID>.tar.zst.age`。
pub(super) const BASE_CIPHER_SUFFIX: &str = ".tar.zst.age";
/// 基础备份 manifest 文件名固定为 `<备份 ID>.tar.zst.age.manifest.json`。
pub(super) const BASE_MANIFEST_SUFFIX: &str = ".tar.zst.age.manifest.json";
/// 基础备份 manifest 的格式名；字段只追加，旧文件继续可读。
pub(super) const BASE_MANIFEST_FORMAT: &str = "auction-base-backup.v1";
/// 本机上传证据文件名固定为 `<备份 ID>.upload.json`。
pub(super) const BASE_RECEIPT_SUFFIX: &str = ".upload.json";
/// 本机上传证据的格式名；只用于避免每 5 分钟重复回读已上传的基础备份。
pub(super) const BASE_RECEIPT_FORMAT: &str = "auction-base-receipt.v1";
/// 本地保留的基础备份份数：与 21 天生命周期规则对齐（决策 49）。
pub(super) const BASE_RETENTION_KEEP: usize = 3;

/// 单个 WAL 段的归档证据；与密文一起构成上传器唯一认可的暂存单元。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ArchiveManifest {
    pub(super) format: String,
    pub(super) segment_name: String,
    /// 源 WAL 文件字节数；与 `source_mtime_ns` 一起证明重复归档的是同一份内容。
    pub(super) source_bytes: u64,
    /// 源 WAL 文件的 mtime（UNIX 纪元纳秒）。PostgreSQL 重试同一段时不变。
    pub(super) source_mtime_ns: u64,
    /// 密文字节数；上传前与本地密文长度核对。
    pub(super) cipher_bytes: u64,
    /// 归档完成时刻；只用于诊断，不参与重复判定。
    pub(super) archived_unix_seconds: u64,
}

impl ArchiveManifest {
    /// 字段闭集与段名一致性；不重算密文摘要（私钥离线，密文不可解）。
    pub(super) fn validate(&self, segment: &str) -> anyhow::Result<()> {
        if self.format != MANIFEST_FORMAT
            || self.segment_name != segment
            || !valid_segment_name(&self.segment_name)
            || self.cipher_bytes == 0
        {
            anyhow::bail!("WAL 归档 manifest 的格式、段名或长度字段无效：{segment}");
        }
        Ok(())
    }
}

/// 上传成功后的持久证据；密文与 manifest 已删除时仍可证明该段已归档。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WalUploadReceipt {
    pub(super) format: String,
    pub(super) segment_name: String,
    pub(super) source_bytes: u64,
    pub(super) source_mtime_ns: u64,
    pub(super) cipher_bytes: u64,
    pub(super) cipher_sha256: String,
    pub(super) remote_key: String,
    pub(super) uploaded_unix_seconds: u64,
}

impl WalUploadReceipt {
    pub(super) fn validate(&self, segment: &str) -> anyhow::Result<()> {
        if self.format != WAL_UPLOAD_FORMAT
            || self.segment_name != segment
            || !valid_segment_name(&self.segment_name)
            || self.source_bytes == 0
            || self.cipher_bytes == 0
            || !is_sha256(&self.cipher_sha256)
            || self.uploaded_unix_seconds == 0
            || self.remote_key.is_empty()
            || self.remote_key.len() > 512
            || self.remote_key.chars().any(char::is_control)
            || !self.remote_key.ends_with(&format!("{segment}{CIPHER_SUFFIX}"))
        {
            anyhow::bail!("WAL 上传证据的格式、段名或摘要字段无效：{segment}");
        }
        Ok(())
    }
}

/// 源 WAL 文件的本地身份；重复归档必须来自同一份内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SourceIdentity {
    pub(super) bytes: u64,
    pub(super) mtime_ns: u64,
}

/// 同一段名的既有暂存状态。
#[derive(Debug)]
pub(super) enum Duplicate {
    /// 既无密文也无 manifest：正常归档。
    Absent,
    /// 上次在发布 manifest 前中断；密文从未被 PostgreSQL 确认，删除后重新归档。
    UnconfirmedCipher(PathBuf),
    /// manifest 与密文齐全且源身份一致：直接视为成功，不重复加密。
    Confirmed(ArchiveManifest),
    /// 已完成远端上传并保留持久证据；密文/manifest 可能已清理，仍视为已归档。
    Uploaded(WalUploadReceipt),
}

/// 已可上传的一对暂存文件。
#[derive(Debug)]
pub(super) struct Staged {
    pub(super) segment_name: String,
    pub(super) cipher: PathBuf,
    pub(super) manifest: PathBuf,
    pub(super) record: ArchiveManifest,
}

/// 一次 `pg_basebackup` 的本地证据；密文只由上传器读取，私钥离线。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BaseManifest {
    pub(super) format: String,
    /// 备份身份，同时也是 `pg_basebackup --label` 与对象键中的目录名。
    pub(super) backup_id: String,
    /// 开始与结束时刻（UNIX 秒）；只用于诊断与排序，不参与上传幂等。
    pub(super) started_unix_seconds: u64,
    pub(super) finished_unix_seconds: u64,
    /// `pg_basebackup` 输出的 tar 流字节数（压缩前）。
    pub(super) plaintext_bytes: u64,
    /// 密文字节数；上传前与本地密文长度核对。
    pub(super) cipher_bytes: u64,
    /// 密文 SHA-256；上传前重算并核对，避免上传已损坏的文件。
    pub(super) cipher_sha256: String,
}

impl BaseManifest {
    /// 字段闭集与备份身份一致性；不重算摘要（扫描时只核对长度，上传时重算）。
    pub(super) fn validate(&self, backup_id: &str) -> anyhow::Result<()> {
        if self.format != BASE_MANIFEST_FORMAT
            || self.backup_id != backup_id
            || !valid_base_id(&self.backup_id)
            || self.plaintext_bytes == 0
            || self.cipher_bytes == 0
            || !is_sha256(&self.cipher_sha256)
            || self.finished_unix_seconds < self.started_unix_seconds
        {
            anyhow::bail!("基础备份 manifest 的格式、身份或长度字段无效：{backup_id}");
        }
        Ok(())
    }
}

/// 已可上传的一组基础备份暂存文件。
#[derive(Debug, Clone)]
pub(super) struct BaseStaged {
    pub(super) backup_id: String,
    pub(super) cipher: PathBuf,
    pub(super) manifest: PathBuf,
    pub(super) record: BaseManifest,
    /// 已有有效本机上传证据;上传器跳过重复的远端完整回读。
    pub(super) uploaded: bool,
    /// 证据绑定的远端目标；与当前配置不一致时必须重新核对。
    pub(super) receipt_destination: Option<String>,
}

/// 本机上传证据：只说明这组密文与 manifest 曾以这些对象摘要完成远端校验。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BaseReceipt {
    pub(super) format: String,
    pub(super) backup_id: String,
    /// endpoint、bucket 与基础备份前缀的组合；只用于绑定上传证据，不含凭据。
    pub(super) destination: String,
    pub(super) uploaded_unix_seconds: u64,
    /// 远端 `completed.json` 的 SHA-256。
    pub(super) completion_sha256: String,
}

impl BaseReceipt {
    pub(super) fn validate(&self, backup_id: &str) -> anyhow::Result<()> {
        if self.format != BASE_RECEIPT_FORMAT
            || self.backup_id != backup_id
            || !valid_base_id(&self.backup_id)
            || self.destination.is_empty()
            || self.destination.len() > 2_048
            || self.destination.chars().any(char::is_control)
            || !is_sha256(&self.completion_sha256)
        {
            anyhow::bail!("基础备份上传证据的格式或身份无效：{backup_id}");
        }
        Ok(())
    }
}

/// 本地保留计划的投影：`retained` 保留在暂存目录，`removable` 由上传器删除。
#[derive(Debug)]
pub(super) struct Retention {
    pub(super) retained: Vec<String>,
    pub(super) removable: Vec<BaseStaged>,
}

/// 无法解析的暂存项；只报告固定原因，不阻断其他项的上传。
#[derive(Debug)]
pub(super) struct InvalidEntry {
    /// WAL 段名或基础备份 ID，取决于所在的扫描集合。
    pub(super) name: String,
    pub(super) reason: String,
}

/// 一次暂存扫描的结果；`incomplete` 是创建进程尚未发布完成的项（WAL 段名或基础备份
/// ID），不是失败。
#[derive(Debug)]
pub(super) struct StagedScan {
    pub(super) ready: Vec<Staged>,
    pub(super) base: Vec<BaseStaged>,
    pub(super) incomplete: Vec<String>,
    pub(super) invalid: Vec<InvalidEntry>,
}

/// PostgreSQL 的 `%f` 只可能是 WAL 段、时间线历史文件或基础备份标签。
pub(super) fn valid_segment_name(name: &str) -> bool {
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
pub(super) fn valid_recipient(recipient: &str) -> bool {
    let bytes = recipient.as_bytes();
    bytes.len() == 62
        && recipient.starts_with("age1")
        && bytes[4..].iter().all(|byte| BECH32.contains(byte))
}

pub(super) fn cipher_path(directory: &Path, segment: &str) -> PathBuf {
    directory.join(format!("{segment}{CIPHER_SUFFIX}"))
}

pub(super) fn manifest_path(directory: &Path, segment: &str) -> PathBuf {
    directory.join(format!("{segment}{MANIFEST_SUFFIX}"))
}

pub(super) fn wal_upload_path(directory: &Path, segment: &str) -> PathBuf {
    directory.join(format!("{segment}{WAL_UPLOAD_SUFFIX}"))
}

/// 基础备份 ID 固定为 `base-YYYYMMDDTHHMMSSZ`；只从 UTC 时刻派生，不含路径分隔符。
pub(super) fn valid_base_id(backup_id: &str) -> bool {
    let Some(stamp) = backup_id.strip_prefix(BASE_ID_PREFIX) else { return false };
    let bytes = stamp.as_bytes();
    bytes.len() == 16
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[8] == b'T'
        && bytes[9..15].iter().all(u8::is_ascii_digit)
        && bytes[15] == b'Z'
}

/// 由当前 UTC 时刻派生备份 ID；同一秒内的重复运行得到同一身份。
pub(super) fn base_id(now: chrono::DateTime<chrono::Utc>) -> String {
    format!("{BASE_ID_PREFIX}{}", now.format("%Y%m%dT%H%M%SZ"))
}

pub(super) fn base_cipher_path(directory: &Path, backup_id: &str) -> PathBuf {
    directory.join(format!("{backup_id}{BASE_CIPHER_SUFFIX}"))
}

pub(super) fn base_manifest_path(directory: &Path, backup_id: &str) -> PathBuf {
    directory.join(format!("{backup_id}{BASE_MANIFEST_SUFFIX}"))
}

pub(super) fn base_receipt_path(directory: &Path, backup_id: &str) -> PathBuf {
    directory.join(format!("{backup_id}{BASE_RECEIPT_SUFFIX}"))
}

/// 文件名对应的基础备份身份；不是基础备份名时返回 None。
pub(super) fn base_id_from_file_name(name: &str) -> Option<&str> {
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
pub(super) fn retention_plan(
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
pub(super) fn require_staging_dir(directory: &Path) -> anyhow::Result<()> {
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
pub(super) fn source_identity(path: &Path) -> anyhow::Result<SourceIdentity> {
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

/// 重复归档只认 manifest 与密文的完整证据；任何不一致都失败关闭，不覆盖。
pub(super) fn classify(
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
pub(super) fn staged(directory: &Path) -> anyhow::Result<StagedScan> {
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
fn read_base_manifest(path: &Path) -> anyhow::Result<BaseManifest> {
    let bytes = files::read_bounded(path, MAX_MANIFEST_BYTES)?;
    serde_json::from_slice(&bytes).context("解析基础备份 manifest 失败")
}

/// 本机上传证据无效或缺失时视为尚未上传：宁可多一次完整回读，也不能跳过真正未上传的备份。
fn read_base_receipt(directory: &Path, backup_id: &str) -> anyhow::Result<Option<BaseReceipt>> {
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
fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn read_wal_upload(path: &Path) -> anyhow::Result<WalUploadReceipt> {
    let bytes = files::read_bounded(path, MAX_MANIFEST_BYTES)?;
    serde_json::from_slice(&bytes).context("解析 WAL 上传证据失败")
}

/// 有界读取 manifest；拒绝符号链接、特殊文件与超出上限的输入。
fn read_manifest(path: &Path) -> anyhow::Result<ArchiveManifest> {
    let bytes = files::read_bounded(path, MAX_MANIFEST_BYTES)?;
    serde_json::from_slice(&bytes).context("解析 WAL 归档 manifest 失败")
}

/// 只接受普通文件；缺失返回 None，符号链接与特殊文件直接失败。
fn regular_or_none(path: &Path) -> anyhow::Result<Option<Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            anyhow::bail!("暂存项必须是普通文件：{}", path.display())
        }
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("读取暂存项失败：{}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        ArchiveManifest, BASE_MANIFEST_FORMAT, BASE_RECEIPT_FORMAT, BaseManifest, BaseReceipt,
        BaseStaged, Duplicate, MANIFEST_FORMAT, SourceIdentity, WAL_UPLOAD_FORMAT,
        WalUploadReceipt, base_cipher_path, base_id, base_id_from_file_name, base_manifest_path,
        base_receipt_path, classify, retention_plan, staged, valid_base_id, valid_recipient,
        valid_segment_name, wal_upload_path,
    };

    #[test]
    fn segment_names_match_postgresql_forms() {
        assert!(valid_segment_name("000000010000000000000001"));
        assert!(valid_segment_name("00000002.history"));
        assert!(valid_segment_name("000000010000000000000001.00000028.backup"));
        assert!(!valid_segment_name("00000001000000000000000"));
        assert!(!valid_segment_name("00000001000000000000000Z"));
        assert!(!valid_segment_name("00000001.history.bak"));
        assert!(!valid_segment_name("000000010000000000000001.zst.age"));
        assert!(!valid_segment_name("../000000010000000000000001"));
    }

    #[test]
    fn recipients_require_age1_shape() {
        let valid = "age1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq";
        assert_eq!(valid.len(), 62);
        assert!(valid_recipient(valid));
        assert!(!valid_recipient("AGE1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQ"));
        assert!(!valid_recipient("age1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqb"));
        assert!(!valid_recipient("--recipient"));
        assert!(!valid_recipient(""));
    }

    #[test]
    fn manifest_validation_binds_segment_and_length() {
        let record = ArchiveManifest {
            format: MANIFEST_FORMAT.to_owned(),
            segment_name: "000000010000000000000001".to_owned(),
            source_bytes: 16 * 1024 * 1024,
            source_mtime_ns: 1,
            cipher_bytes: 4096,
            archived_unix_seconds: 2,
        };
        assert!(record.validate("000000010000000000000001").is_ok());
        assert!(record.validate("000000010000000000000002").is_err());

        let mut empty = record.clone();
        empty.cipher_bytes = 0;
        assert!(empty.validate("000000010000000000000001").is_err());

        let mut wrong_format = record;
        wrong_format.format = "auction-wal-archive.v0".to_owned();
        assert!(wrong_format.validate("000000010000000000000001").is_err());
    }

    #[test]
    fn durable_upload_proof_classifies_without_cipher_or_manifest() {
        let directory =
            std::env::temp_dir().join(format!("auction-wal-upload-proof-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).expect("建立临时暂存目录");
        let segment = "000000010000000000000001";
        let source = SourceIdentity { bytes: 128, mtime_ns: 7 };
        let receipt = WalUploadReceipt {
            format: WAL_UPLOAD_FORMAT.to_owned(),
            segment_name: segment.to_owned(),
            source_bytes: source.bytes,
            source_mtime_ns: source.mtime_ns,
            cipher_bytes: 64,
            cipher_sha256: "a".repeat(64),
            remote_key: format!("wal/{segment}.zst.age"),
            uploaded_unix_seconds: 1,
        };
        std::fs::write(
            wal_upload_path(&directory, segment),
            serde_json::to_vec(&receipt).expect("序列化上传证据"),
        )
        .expect("写入上传证据");

        match classify(&directory, segment, &source).expect("读取上传证据") {
            Duplicate::Uploaded(record) => assert_eq!(record.cipher_bytes, 64),
            other => panic!("应识别为已上传：{other:?}"),
        }
        let changed = SourceIdentity { bytes: 128, mtime_ns: 8 };
        assert!(classify(&directory, segment, &changed).is_err());
        let _ = std::fs::remove_dir_all(&directory);
    }

    fn base_record(backup_id: &str) -> BaseManifest {
        BaseManifest {
            format: BASE_MANIFEST_FORMAT.to_owned(),
            backup_id: backup_id.to_owned(),
            started_unix_seconds: 10,
            finished_unix_seconds: 20,
            plaintext_bytes: 1024,
            cipher_bytes: 512,
            cipher_sha256: "0".repeat(64),
        }
    }

    fn staged_base(backup_id: &str) -> BaseStaged {
        let directory = Path::new("/nonexistent");
        BaseStaged {
            backup_id: backup_id.to_owned(),
            cipher: base_cipher_path(directory, backup_id),
            manifest: base_manifest_path(directory, backup_id),
            record: base_record(backup_id),
            uploaded: false,
            receipt_destination: None,
        }
    }

    #[test]
    fn base_ids_follow_derived_timestamp_form() {
        assert!(valid_base_id("base-20261104T031500Z"));
        assert_eq!(
            base_id(
                chrono::DateTime::parse_from_rfc3339("2026-11-04T03:15:00Z")
                    .expect("解析时刻")
                    .with_timezone(&chrono::Utc)
            ),
            "base-20261104T031500Z"
        );
        assert!(!valid_base_id("base-20261104T031500"));
        assert!(!valid_base_id("base-2026110T031500Z"));
        assert!(!valid_base_id("base-20261104t031500Z"));
        assert!(!valid_base_id("000000010000000000000001"));
        assert!(!valid_base_id("../base-20261104T031500Z"));

        assert_eq!(
            base_id_from_file_name("base-20261104T031500Z.tar.zst.age"),
            Some("base-20261104T031500Z")
        );
        assert_eq!(
            base_id_from_file_name("base-20261104T031500Z.tar.zst.age.manifest.json"),
            Some("base-20261104T031500Z")
        );
        assert_eq!(base_id_from_file_name("000000010000000000000001.zst.age"), None);
        assert_eq!(
            base_id_from_file_name("base-20261104T031500Z.upload.json"),
            Some("base-20261104T031500Z")
        );
        assert_eq!(base_id_from_file_name("base-backup.lock"), None);
    }

    #[test]
    fn base_manifest_validation_binds_identity_shape_and_length() {
        let record = base_record("base-20261104T031500Z");
        assert!(record.validate("base-20261104T031500Z").is_ok());
        assert!(record.validate("base-20261111T031500Z").is_err());

        let mut empty = record.clone();
        empty.cipher_bytes = 0;
        assert!(empty.validate("base-20261104T031500Z").is_err());

        let mut unordered = record.clone();
        unordered.finished_unix_seconds = 1;
        assert!(unordered.validate("base-20261104T031500Z").is_err());

        let mut bad_digest = record;
        bad_digest.cipher_sha256 = "z".repeat(64);
        assert!(bad_digest.validate("base-20261104T031500Z").is_err());
    }

    #[test]
    fn retention_keeps_newest_sets_and_extends_for_failed_ids() {
        let complete = vec![
            staged_base("base-20261014T031500Z"),
            staged_base("base-20261021T031500Z"),
            staged_base("base-20261028T031500Z"),
            staged_base("base-20261104T031500Z"),
        ];
        let plan = retention_plan(complete.clone(), 3, &[]);
        assert_eq!(plan.retained.len(), 3);
        assert_eq!(plan.removable.len(), 1);
        assert_eq!(plan.removable[0].backup_id, "base-20261014T031500Z");
        assert_eq!(plan.retained[0], "base-20261104T031500Z");

        let plan = retention_plan(complete, 3, &["base-20261014T031500Z".to_owned()]);
        assert!(plan.removable.is_empty());
        assert_eq!(plan.retained.len(), 4);

        // 少于保留份数时不删除任何项。
        let plan = retention_plan(vec![staged_base("base-20261104T031500Z")], 3, &[]);
        assert!(plan.removable.is_empty());
    }

    #[test]
    fn staged_marks_base_sets_uploaded_only_with_valid_receipt() {
        use std::fs;

        let root = std::env::temp_dir().join(format!(
            "auction-wal-staged-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("时钟")
                .as_nanos()
        ));
        fs::create_dir(&root).expect("创建测试目录");
        let id = "base-20261104T031500Z";
        fs::write(base_cipher_path(&root, id), b"cipher").expect("写入密文");
        let record = base_record(id);
        let record = BaseManifest { cipher_bytes: 6, plaintext_bytes: 16, ..record };
        fs::write(base_manifest_path(&root, id), serde_json::to_vec(&record).expect("序列化"))
            .expect("写入 manifest");

        let scan = staged(&root).expect("首次扫描");
        assert_eq!(scan.base.len(), 1);
        assert!(!scan.base[0].uploaded);

        let receipt = BaseReceipt {
            format: BASE_RECEIPT_FORMAT.to_owned(),
            backup_id: id.to_owned(),
            destination: "https://example.invalid\u{241f}bucket\u{241f}prefix/base".to_owned(),
            uploaded_unix_seconds: 3,
            completion_sha256: "1".repeat(64),
        };
        let receipt_path = base_receipt_path(&root, id);
        fs::write(&receipt_path, serde_json::to_vec(&receipt).expect("序列化证据"))
            .expect("写入证据");
        let scan = staged(&root).expect("带证据扫描");
        assert!(scan.base[0].uploaded);
        assert_eq!(
            scan.base[0].receipt_destination.as_deref(),
            Some("https://example.invalid\u{241f}bucket\u{241f}prefix/base")
        );

        // 身份不符的证据不能把未上传的组当成已上传。
        let wrong = BaseReceipt { backup_id: "base-20261111T031500Z".to_owned(), ..receipt };
        fs::write(&receipt_path, serde_json::to_vec(&wrong).expect("序列化证据"))
            .expect("覆盖证据");
        let scan = staged(&root).expect("错误证据扫描");
        assert!(!scan.base[0].uploaded);

        // 损坏的证据按未上传处理，不使整次扫描失败。
        fs::write(&receipt_path, b"not-json").expect("写入损坏证据");
        let scan = staged(&root).expect("损坏证据扫描");
        assert!(!scan.base[0].uploaded);

        fs::remove_dir_all(&root).expect("清理测试目录");
    }
}
