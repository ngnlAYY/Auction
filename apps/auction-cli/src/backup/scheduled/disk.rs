//! 只读检查备份文件系统的非特权可用块；两个阈值必须同时满足，不以清理保留点绕过门禁。

use std::{mem::MaybeUninit, os::fd::AsRawFd};

use anyhow::Context;
use auction_config::BackupConfig;
use serde::{Deserialize, Serialize};

use super::filesystem::PrivateDirectory;

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
    let mut statistics = MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY：fstatvfs 只写入正确尺寸的缓冲区；仅在返回成功后读取已初始化的值。
    if unsafe { libc::fstatvfs(directory.directory.as_raw_fd(), statistics.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error()).context("读取磁盘水位失败");
    }
    // SAFETY：上面的 fstatvfs 已成功填充整个结构体。
    let statistics = unsafe { statistics.assume_init() };
    let total_bytes =
        statistics.f_blocks.checked_mul(statistics.f_frsize).context("文件系统大小溢出")?;
    let available_bytes =
        statistics.f_bavail.checked_mul(statistics.f_frsize).context("文件系统可用大小溢出")?;
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
