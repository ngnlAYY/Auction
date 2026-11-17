//! 备份 manifest 与摘要：manifest 绑定 archive 文件名、SHA-256、库名摘要与 schema 版本。
//! 格式名沿用 `auction-postgres-backup.v1`，以前的运维工具生成的备份可直接校验与恢复。

use std::path::{Path, PathBuf};

use anyhow::Context;
use auction_time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::support::digest::{is_lower_hex, sha256_bytes};
use crate::support::files;

pub(super) const FORMAT: &str = "auction-postgres-backup.v1";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BackupManifest {
    pub(crate) format: String,
    pub(crate) archive_file_name: String,
    pub(crate) database_name_sha256: String,
    pub(crate) archive_sha256: String,
    pub(crate) schema_version: i64,
    pub(crate) created_at: Timestamp,
}

impl BackupManifest {
    /// 读取 manifest，并返回同一份原始字节的摘要作为恢复回执证据。
    pub(super) fn read(path: &Path) -> anyhow::Result<(Self, String)> {
        let bytes = files::read_bounded(path, MAX_MANIFEST_BYTES)?;
        let manifest: Self = serde_json::from_slice(&bytes).context("解析备份 manifest 失败")?;
        Ok((manifest, sha256_bytes(&bytes)))
    }

    /// 字段闭集与库名摘要校验；archive 必须与 manifest 位于同一目录。
    pub(super) fn validate(&self, manifest_path: &Path, database: &str) -> anyhow::Result<PathBuf> {
        let name = &self.archive_file_name;
        if self.format != FORMAT
            || self.schema_version <= 0
            || name.is_empty()
            || name.trim() != name
            || name.contains(['/', '\\'])
            || name.chars().any(char::is_control)
            || !name.ends_with(".dump")
            || !is_lower_hex(&self.database_name_sha256, 64)
            || !is_lower_hex(&self.archive_sha256, 64)
        {
            anyhow::bail!("备份 manifest 的格式、文件名或摘要字段无效");
        }
        if self.database_name_sha256 != sha256_bytes(database.as_bytes()) {
            anyhow::bail!("--database 与备份 manifest 记录的数据库不一致");
        }
        let manifest_name = manifest_path.file_name().and_then(|name| name.to_str());
        if manifest_name != Some(manifest_file_name(name).as_str()) {
            anyhow::bail!("manifest 文件名必须是 `<archive>.manifest`");
        }
        Ok(manifest_path.parent().context("manifest 缺少父目录")?.join(name))
    }
}

pub(super) fn manifest_file_name(archive_file_name: &str) -> String {
    format!("{archive_file_name}.manifest")
}
