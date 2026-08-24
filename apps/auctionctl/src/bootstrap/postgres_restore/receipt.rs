use super::*;

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(super) struct PostgresRestoreReceipt {
    pub(super) format: &'static str,
    pub(super) restored_at: DateTime<Utc>,
    pub(super) database_name: String,
    pub(super) database_name_sha256: String,
    pub(super) archive_file_name: String,
    pub(super) archive_sha256: String,
    pub(super) manifest_sha256: String,
    pub(super) schema_version: i64,
    pub(super) single_transaction: bool,
    pub(super) ownership_restored: bool,
    pub(super) privileges_restored: bool,
}

pub(super) struct RestoreReceiptReservation {
    target_path: PathBuf,
    temporary_path: PathBuf,
    published: bool,
}

impl RestoreReceiptReservation {
    pub(super) fn reserve(target_path: &Path) -> anyhow::Result<Self> {
        validate_private_evidence_path(target_path)?;
        require_absent_artifact(target_path, "restore receipt")?;
        let parent = target_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("restore receipt parent is missing"))?;
        let metadata = fs::symlink_metadata(parent)
            .with_context(|| format!("read restore receipt parent: {}", parent.display()))?;
        if !metadata.file_type().is_dir() {
            anyhow::bail!("restore receipt parent is not a directory: {}", parent.display());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o022 != 0 {
                anyhow::bail!(
                    "restore receipt parent is group/other writable: {}",
                    parent.display()
                );
            }
        }
        let temporary_path =
            create_private_temp_path(target_path, "auction-postgres-restore-receipt")?;
        Ok(Self { target_path: target_path.to_owned(), temporary_path, published: false })
    }

    pub(super) fn publish(mut self, contents: &[u8]) -> anyhow::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&self.temporary_path)
            .with_context(|| {
                format!("open restore receipt temporary file: {}", self.temporary_path.display())
            })?;
        file.write_all(contents).context("write PostgreSQL restore receipt")?;
        file.write_all(b"\n").context("terminate PostgreSQL restore receipt")?;
        file.sync_all().context("sync PostgreSQL restore receipt")?;
        drop(file);

        require_absent_artifact(&self.target_path, "restore receipt")?;
        fs::hard_link(&self.temporary_path, &self.target_path).with_context(|| {
            format!("publish PostgreSQL restore receipt: {}", self.target_path.display())
        })?;
        fs::remove_file(&self.temporary_path).with_context(|| {
            format!(
                "restore committed and receipt published, but temporary receipt cleanup failed: {}",
                self.temporary_path.display()
            )
        })?;
        self.published = true;
        Ok(())
    }
}

impl Drop for RestoreReceiptReservation {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.temporary_path);
        }
    }
}
