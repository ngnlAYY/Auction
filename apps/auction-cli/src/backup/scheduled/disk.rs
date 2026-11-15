//! 只读检查备份文件系统的非特权可用块；两个阈值必须同时满足，不以清理保留点绕过门禁。

use auction_config::BackupConfig;
use serde::{Deserialize, Serialize};

use crate::support::files::PrivateDirectory;

/// 回执中的磁盘检查快照，百分比比较使用整数，避免浮点取整放宽门禁。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DiskSnapshot {
    pub total_bytes: u64,
    pub available_bytes: u64,
    /// 尚未写出的密文空间上界；已存在的暂存由文件系统可用块自然计入。
    #[serde(default)]
    pub reserved_bytes: u64,
    pub min_free_bytes: u64,
    pub min_free_percent: u8,
    pub passed: bool,
}

pub(super) fn inspect(
    directory: &PrivateDirectory,
    config: &BackupConfig,
    reserved_bytes: u64,
) -> anyhow::Result<DiskSnapshot> {
    let usage = directory.usage()?;
    let total_bytes = usage.total_bytes;
    let available_bytes = usage.available_bytes;
    let remaining = available_bytes.saturating_sub(reserved_bytes);
    let passed = total_bytes != 0
        && available_bytes >= reserved_bytes
        && remaining >= config.min_free_bytes
        && u128::from(remaining) * 100
            >= u128::from(total_bytes) * u128::from(config.min_free_percent);
    Ok(DiskSnapshot {
        total_bytes,
        available_bytes,
        reserved_bytes,
        min_free_bytes: config.min_free_bytes,
        min_free_percent: config.min_free_percent,
        passed,
    })
}
