//! PostgreSQL 重建回执的不可覆盖私有文件发布器。

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::Context;

use super::super::{
    create_private_temp_path, require_absent_artifact, validate_private_evidence_path,
};

/// 在 mutation 前预留私有临时文件，成功或失败后只能发布一次目标回执。
pub(super) struct RebuildReceiptReservation {
    target_path: PathBuf,
    temporary_path: PathBuf,
    published: bool,
}

impl RebuildReceiptReservation {
    pub(super) fn reserve(target_path: &Path) -> anyhow::Result<Self> {
        validate_private_evidence_path(target_path)?;
        require_absent_artifact(target_path, "rebuild receipt")?;
        let parent = target_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("rebuild receipt parent is missing"))?;
        let metadata = fs::symlink_metadata(parent)
            .with_context(|| format!("read rebuild receipt parent: {}", parent.display()))?;
        if !metadata.file_type().is_dir() {
            anyhow::bail!("rebuild receipt parent is not a directory: {}", parent.display());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o022 != 0 {
                anyhow::bail!(
                    "rebuild receipt parent is group/other writable: {}",
                    parent.display()
                );
            }
        }
        let temporary_path =
            create_private_temp_path(target_path, "auction-postgres-rebuild-receipt")?;
        Ok(Self { target_path: target_path.to_owned(), temporary_path, published: false })
    }

    pub(super) fn publish(mut self, contents: &[u8]) -> anyhow::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&self.temporary_path)
            .with_context(|| {
                format!("open rebuild receipt temporary file: {}", self.temporary_path.display())
            })?;
        file.write_all(contents).context("write PostgreSQL rebuild receipt")?;
        file.write_all(b"\n").context("terminate PostgreSQL rebuild receipt")?;
        file.sync_all().context("sync PostgreSQL rebuild receipt")?;
        drop(file);

        require_absent_artifact(&self.target_path, "rebuild receipt")?;
        fs::hard_link(&self.temporary_path, &self.target_path).with_context(|| {
            format!("publish PostgreSQL rebuild receipt: {}", self.target_path.display())
        })?;
        fs::remove_file(&self.temporary_path).with_context(|| {
            format!(
                "rebuild receipt published but temporary cleanup failed: {}",
                self.temporary_path.display()
            )
        })?;
        self.published = true;
        Ok(())
    }
}

impl Drop for RebuildReceiptReservation {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.temporary_path);
        }
    }
}
