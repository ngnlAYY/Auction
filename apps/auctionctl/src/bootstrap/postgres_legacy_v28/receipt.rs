//! PostgreSQL v28 接管 receipt 的私有预留、原子发布和失败清理。

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::Context;

use super::super::{
    create_private_temp_path, require_absent_artifact, validate_private_evidence_path,
};

pub(super) struct AdoptionReceiptReservation {
    target_path: PathBuf,
    temporary_path: PathBuf,
    published: bool,
}

impl AdoptionReceiptReservation {
    pub(super) fn reserve(target_path: &Path) -> anyhow::Result<Self> {
        validate_private_evidence_path(target_path)?;
        require_absent_artifact(target_path, "legacy v28 adoption receipt")?;
        let parent = target_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("adoption receipt parent is missing"))?;
        let metadata = fs::symlink_metadata(parent)
            .with_context(|| format!("read adoption receipt parent: {}", parent.display()))?;
        if !metadata.file_type().is_dir() {
            anyhow::bail!("adoption receipt parent is not a directory: {}", parent.display());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o022 != 0 {
                anyhow::bail!(
                    "adoption receipt parent is group/other writable: {}",
                    parent.display()
                );
            }
        }
        let temporary_path =
            create_private_temp_path(target_path, "auction-postgres-legacy-v28-receipt")?;
        Ok(Self { target_path: target_path.to_owned(), temporary_path, published: false })
    }

    pub(super) fn publish(mut self, contents: &[u8]) -> anyhow::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&self.temporary_path)
            .with_context(|| {
                format!("open adoption receipt temporary file: {}", self.temporary_path.display())
            })?;
        file.write_all(contents).context("write legacy v28 adoption receipt")?;
        file.write_all(b"\n").context("terminate legacy v28 adoption receipt")?;
        file.sync_all().context("sync legacy v28 adoption receipt")?;
        drop(file);

        require_absent_artifact(&self.target_path, "legacy v28 adoption receipt")?;
        fs::hard_link(&self.temporary_path, &self.target_path).with_context(|| {
            format!("publish legacy v28 adoption receipt: {}", self.target_path.display())
        })?;
        fs::remove_file(&self.temporary_path).with_context(|| {
            format!(
                "adoption committed and receipt published, but temporary receipt cleanup failed: {}",
                self.temporary_path.display()
            )
        })?;
        self.published = true;
        Ok(())
    }
}

impl Drop for AdoptionReceiptReservation {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.temporary_path);
        }
    }
}
