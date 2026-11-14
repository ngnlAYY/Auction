//! WAL 暂存命名、归档 manifest、每周基础备份 manifest 与重复判定；只描述本地证据，
//! 不访问数据库或 R2。
//!
//! 暂存目录由 PostgreSQL 服务账号写入、由上传账号读取并删除，因此不能用
//! `PrivateDirectory`（它要求执行账号独占 0700）；这里只要求目录非符号链接且
//! 禁止 other 写入，具体 owner/group 由部署方按 RECOVERY.md 设置。

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

mod manifest;
mod paths;
mod scan;
#[cfg(test)]
mod tests;
pub(super) use manifest::{
    ArchiveManifest, BaseManifest, BaseReceipt, BaseStaged, Duplicate, Retention, SourceIdentity,
    Staged, WalUploadReceipt,
};
pub(super) use paths::{
    base_cipher_path, base_id, base_manifest_path, base_receipt_path, cipher_path, manifest_path,
    require_staging_dir, retention_plan, source_identity, valid_recipient, valid_segment_name,
    wal_upload_path,
};
use scan::*;
pub(super) use scan::{classify, staged};
