//! WAL 与基础备份的清单、上传回执、暂存与保留模型。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::paths::{valid_base_id, valid_segment_name};
use super::{
    BASE_MANIFEST_FORMAT, BASE_RECEIPT_FORMAT, CIPHER_SUFFIX, MANIFEST_FORMAT, WAL_UPLOAD_FORMAT,
    is_sha256,
};

/// 单个 WAL 段的归档证据；与密文一起构成上传器唯一认可的暂存单元。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveManifest {
    pub(crate) format: String,
    pub(crate) segment_name: String,
    /// 源 WAL 文件字节数；与 `source_mtime_ns` 一起证明重复归档的是同一份内容。
    pub(crate) source_bytes: u64,
    /// 源 WAL 文件的 mtime（UNIX 纪元纳秒）。PostgreSQL 重试同一段时不变。
    pub(crate) source_mtime_ns: u64,
    /// 密文字节数；上传前与本地密文长度核对。
    pub(crate) cipher_bytes: u64,
    /// 归档完成时刻；只用于诊断，不参与重复判定。
    pub(crate) archived_unix_seconds: u64,
}

impl ArchiveManifest {
    /// 字段闭集与段名一致性；不重算密文摘要（私钥离线，密文不可解）。
    pub(crate) fn validate(&self, segment: &str) -> anyhow::Result<()> {
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
pub(crate) struct WalUploadReceipt {
    pub(crate) format: String,
    pub(crate) segment_name: String,
    pub(crate) source_bytes: u64,
    pub(crate) source_mtime_ns: u64,
    pub(crate) cipher_bytes: u64,
    pub(crate) cipher_sha256: String,
    pub(crate) remote_key: String,
    pub(crate) uploaded_unix_seconds: u64,
}

impl WalUploadReceipt {
    pub(crate) fn validate(&self, segment: &str) -> anyhow::Result<()> {
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
pub(crate) struct SourceIdentity {
    pub(crate) bytes: u64,
    pub(crate) mtime_ns: u64,
}

/// 同一段名的既有暂存状态。
#[derive(Debug)]
pub(crate) enum Duplicate {
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
pub(crate) struct Staged {
    pub(crate) segment_name: String,
    pub(crate) cipher: PathBuf,
    pub(crate) manifest: PathBuf,
    pub(crate) record: ArchiveManifest,
}

/// 一次 `pg_basebackup` 的本地证据；密文只由上传器读取，私钥离线。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BaseManifest {
    pub(crate) format: String,
    /// 备份身份，同时也是 `pg_basebackup --label` 与对象键中的目录名。
    pub(crate) backup_id: String,
    /// 开始与结束时刻（UNIX 秒）；只用于诊断与排序，不参与上传幂等。
    pub(crate) started_unix_seconds: u64,
    pub(crate) finished_unix_seconds: u64,
    /// `pg_basebackup` 输出的 tar 流字节数（压缩前）。
    pub(crate) plaintext_bytes: u64,
    /// 密文字节数；上传前与本地密文长度核对。
    pub(crate) cipher_bytes: u64,
    /// 密文 SHA-256；上传前重算并核对，避免上传已损坏的文件。
    pub(crate) cipher_sha256: String,
}

impl BaseManifest {
    /// 字段闭集与备份身份一致性；不重算摘要（扫描时只核对长度，上传时重算）。
    pub(crate) fn validate(&self, backup_id: &str) -> anyhow::Result<()> {
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
pub(crate) struct BaseStaged {
    pub(crate) backup_id: String,
    pub(crate) cipher: PathBuf,
    pub(crate) manifest: PathBuf,
    pub(crate) record: BaseManifest,
    /// 已有有效本机上传证据;上传器跳过重复的远端完整回读。
    pub(crate) uploaded: bool,
    /// 证据绑定的远端目标；与当前配置不一致时必须重新核对。
    pub(crate) receipt_destination: Option<String>,
}

/// 本机上传证据：只说明这组密文与 manifest 曾以这些对象摘要完成远端校验。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BaseReceipt {
    pub(crate) format: String,
    pub(crate) backup_id: String,
    /// endpoint、bucket 与基础备份前缀的组合；只用于绑定上传证据，不含凭据。
    pub(crate) destination: String,
    pub(crate) uploaded_unix_seconds: u64,
    /// 远端 `completed.json` 的 SHA-256。
    pub(crate) completion_sha256: String,
}

impl BaseReceipt {
    pub(crate) fn validate(&self, backup_id: &str) -> anyhow::Result<()> {
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
pub(crate) struct Retention {
    pub(crate) retained: Vec<String>,
    pub(crate) removable: Vec<BaseStaged>,
}

/// 无法解析的暂存项；只报告固定原因，不阻断其他项的上传。
#[derive(Debug)]
pub(crate) struct InvalidEntry {
    /// WAL 段名或基础备份 ID，取决于所在的扫描集合。
    pub(crate) name: String,
    pub(crate) reason: String,
}

/// 一次暂存扫描的结果；`incomplete` 是创建进程尚未发布完成的项（WAL 段名或基础备份
/// ID），不是失败。
#[derive(Debug)]
pub(crate) struct StagedScan {
    pub(crate) ready: Vec<Staged>,
    pub(crate) base: Vec<BaseStaged>,
    pub(crate) incomplete: Vec<String>,
    pub(crate) invalid: Vec<InvalidEntry>,
}
