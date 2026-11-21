//! 文件系统容量：一次 `fstatvfs` 读取总字节与非特权可用字节。定时备份水位、主机告警报告与
//! 部署空间门禁共用；阈值判定归各调用方。

use std::{fs::File, mem::MaybeUninit, os::fd::AsRawFd, path::Path};

use anyhow::Context;

/// Linux Btrfs superblock magic in `statfs.f_type`.
pub(crate) const BTRFS_FILESYSTEM_TYPE: i64 = 0x9123_683e;

/// 文件系统的总字节、非特权可用字节与 Linux 文件系统类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiskUsage {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub filesystem_type: i64,
}

/// 读取已打开描述符所在文件系统的容量。
pub(crate) fn usage(file: &File) -> anyhow::Result<DiskUsage> {
    let mut statistics = MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY：fstatvfs 只写入正确尺寸的缓冲区；仅在返回成功后读取已初始化的值。
    if unsafe { libc::fstatvfs(file.as_raw_fd(), statistics.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error()).context("读取磁盘水位失败");
    }
    // SAFETY：上面的 fstatvfs 已成功填充整个结构体。
    let statistics = unsafe { statistics.assume_init() };
    let mut filesystem = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY：fstatfs 只写入正确尺寸的缓冲区；仅在返回成功后读取已初始化的值。
    if unsafe { libc::fstatfs(file.as_raw_fd(), filesystem.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error()).context("读取文件系统类型失败");
    }
    // SAFETY：上面的 fstatfs 已成功填充整个结构体。
    let filesystem = unsafe { filesystem.assume_init() };
    Ok(DiskUsage {
        total_bytes: statistics
            .f_blocks
            .checked_mul(statistics.f_frsize)
            .context("文件系统大小溢出")?,
        available_bytes: statistics
            .f_bavail
            .checked_mul(statistics.f_frsize)
            .context("文件系统可用大小溢出")?,
        filesystem_type: filesystem.f_type,
    })
}

/// 读取目录所在文件系统的容量；路径必须是目录。
pub(crate) fn disk_usage(directory: &Path) -> anyhow::Result<DiskUsage> {
    let file =
        File::open(directory).with_context(|| format!("打开目录失败：{}", directory.display()))?;
    let metadata = file.metadata().context("读取目录属性失败")?;
    if !metadata.is_dir() {
        anyhow::bail!("路径不是目录：{}", directory.display());
    }
    usage(&file)
}

#[cfg(test)]
mod tests {
    use super::disk_usage;

    #[test]
    fn disk_usage_reports_the_directory_filesystem() {
        let usage = disk_usage(&std::env::temp_dir()).expect("读取文件系统水位");
        assert!(usage.total_bytes > 0);
        assert!(usage.available_bytes <= usage.total_bytes);
    }

    #[test]
    fn disk_usage_rejects_missing_paths_and_files() {
        let missing = std::env::temp_dir().join(format!("auction-disk-{}", uuid::Uuid::new_v4()));
        assert!(disk_usage(&missing).is_err());
        let file = std::env::temp_dir().join(format!("auction-disk-{}", uuid::Uuid::new_v4()));
        std::fs::write(&file, b"x").expect("写入文件");
        let result = disk_usage(&file);
        std::fs::remove_file(&file).expect("清理文件");
        assert!(result.is_err(), "普通文件不是目录");
    }
}
