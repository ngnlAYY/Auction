//! 身份核验后的私有目录项及子目录删除。

use std::os::{fd::AsRawFd, unix::fs::MetadataExt};

use anyhow::Context;
use uuid::Uuid;

use super::super::path_helpers::{name_component, restore_quarantined_directory};
use super::entry_lock::lock_directory_entries;
use super::{FileIdentity, PrivateDirectory};

impl PrivateDirectory {
    pub(crate) fn remove(&self, name: &str, expected: FileIdentity) -> anyhow::Result<()> {
        let _entry_lock = lock_directory_entries(&self.directory)?;
        let source = name_component(name)?;
        let quarantine = format!(".remove-{}.partial", Uuid::new_v4());
        let target = name_component(&quarantine)?;
        let directory = self.directory.as_raw_fd();
        // Atomically move the current directory entry to a private unpredictable name. If it was
        // replaced after the caller's identity check, it can be restored without deleting it.
        // SAFETY: both names are validated single components and the pinned directory fd is live.
        if unsafe {
            libc::renameat2(
                directory,
                source.as_ptr(),
                directory,
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error()).context("隔离待删除私有文件失败");
        }
        let quarantined_file = self.open(&quarantine);
        let identity = match &quarantined_file {
            Ok(file) => file
                .metadata()
                .map(|metadata| FileIdentity::of(&metadata))
                .map_err(anyhow::Error::from),
            Err(error) => Err(anyhow::anyhow!("隔离私有文件无法读取：{error}")),
        };
        if !matches!(identity, Ok(actual) if actual == expected) {
            // Restore without replacing a name another writer may have created meanwhile. If it
            // cannot be restored, keep the quarantined entry for operator recovery.
            // SAFETY: both names remain validated single components under the same pinned fd.
            if unsafe {
                libc::renameat2(
                    directory,
                    target.as_ptr(),
                    directory,
                    source.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            } != 0
            {
                return Err(anyhow::anyhow!(
                    "私有文件身份变化，且无法恢复隔离文件 {}：{}",
                    quarantine,
                    std::io::Error::last_os_error()
                ));
            }
            anyhow::bail!("私有文件身份或大小已变化，停止删除");
        }
        let _quarantined_file = quarantined_file?;
        self.unlink(&quarantine)?;
        self.sync()
    }

    pub(crate) fn remove_child(&self, child: &Self) -> anyhow::Result<()> {
        let _child_entry_lock = lock_directory_entries(&child.directory)?;
        let _parent_entry_lock = lock_directory_entries(&self.directory)?;
        let name =
            child.path.file_name().and_then(|name| name.to_str()).context("子目录名称无效")?;
        if child.path != self.path.join(name) {
            anyhow::bail!("只允许删除当前私有目录下的直接子目录");
        }
        let source = name_component(name)?;
        let quarantine = format!(".remove-{}.partial", Uuid::new_v4());
        let target = name_component(&quarantine)?;
        let directory = self.directory.as_raw_fd();
        let expected = child.directory.metadata()?;
        // 固定父目录描述符内先原子隔离目录项，再验证句柄身份与空目录条件。
        // SAFETY：目录描述符有效，两个名称均为经过校验的单路径分量。
        if unsafe {
            libc::renameat2(
                directory,
                source.as_ptr(),
                directory,
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error()).context("隔离待删除私有子目录失败");
        }
        let actual = self
            .open_at(&quarantine, libc::O_RDONLY | libc::O_DIRECTORY, 0)
            .and_then(|entry| entry.metadata().map_err(anyhow::Error::from));
        if !matches!(
            actual,
            Ok(metadata)
                if metadata.dev() == expected.dev() && metadata.ino() == expected.ino()
        ) {
            restore_quarantined_directory(directory, &target, &source, &quarantine)?;
            anyhow::bail!("隔离后的子目录身份变化，已停止删除");
        }
        let empty = match child.names() {
            Ok(names) => names.is_empty(),
            Err(error) => {
                restore_quarantined_directory(directory, &target, &source, &quarantine)?;
                return Err(error);
            }
        };
        if !empty {
            restore_quarantined_directory(directory, &target, &source, &quarantine)?;
            anyhow::bail!("隔离后的私有子目录不为空，已停止删除");
        }
        // SAFETY：隔离名仍位于固定父目录下；目录锁和 owner-only 权限排除非协作外部写入。
        if unsafe { libc::unlinkat(directory, target.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
            let error = std::io::Error::last_os_error();
            restore_quarantined_directory(directory, &target, &source, &quarantine)?;
            return Err(error).context("删除已核验的空私有子目录失败");
        }
        self.sync()
    }
}
