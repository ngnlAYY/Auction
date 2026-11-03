//! 备份 manifest 与摘要：manifest 绑定 archive 文件名、SHA-256、库名摘要与 schema 版本。
//! 格式名沿用 `auction-postgres-backup.v1`，以前的运维工具生成的备份可直接校验与恢复。

use std::path::{Path, PathBuf};

use anyhow::Context;
use auction_application::lower_hex;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

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
    pub(crate) created_at: DateTime<Utc>,
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
            || !is_sha256(&self.database_name_sha256)
            || !is_sha256(&self.archive_sha256)
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

pub(crate) fn sha256_bytes(bytes: &[u8]) -> String {
    lower_hex(&Sha256::digest(bytes))
}

/// 流式计算普通文件摘要；拒绝符号链接与特殊文件。
pub(super) async fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file = tokio::fs::File::from_std(files::open_regular(path)?);
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let count = file.read(&mut buffer).await.context("读取 archive 失败")?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(lower_hex(&digest.finalize()))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}
