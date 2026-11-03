//! 状态锁内的待上传检查点和只追加完成事实；不修改任何已发布的运行终态回执。

use anyhow::Context;
use auction_config::BackupOffsiteConfig;
use uuid::Uuid;

use super::super::{
    filesystem::{FileIdentity, PrivateDirectory},
    journal::Journal,
    model::RunReceipt,
};
use super::model::{COMPLETED_FORMAT, CompletedRun, Pending};
use crate::backup::manifest::sha256_bytes;

pub(super) struct State {
    pub pending: PrivateDirectory,
    pub completed: PrivateDirectory,
    pub months: PrivateDirectory,
}

impl State {
    pub fn open(journal: &Journal) -> anyhow::Result<Self> {
        let root = journal.state.child("offsite")?;
        Ok(Self {
            pending: root.child("pending")?,
            completed: root.child("completed")?,
            months: root.child("months")?,
        })
    }

    pub fn save(&self, pending: &Pending) -> anyhow::Result<()> {
        self.pending.write(&name(pending.run_id), &serde_json::to_vec_pretty(pending)?, true)
    }

    pub fn finish(&self, completed: &CompletedRun) -> anyhow::Result<()> {
        let file = name(completed.run_id);
        let bytes = serde_json::to_vec_pretty(completed)?;
        match self.completed.read(&file) {
            Ok(existing) if existing == bytes => Ok(()),
            Ok(_) => anyhow::bail!("本地副本完成事实冲突"),
            Err(error) if missing(&error) => self.completed.write(&file, &bytes, false),
            Err(error) => Err(error),
        }
    }

    pub fn remove_pending(&self, run_id: Uuid) -> anyhow::Result<()> {
        let file = name(run_id);
        let identity = FileIdentity::of(&self.pending.open(&file)?.metadata()?);
        self.pending.remove(&file, identity)
    }
}

pub(super) fn destination(config: &BackupOffsiteConfig) -> anyhow::Result<String> {
    Ok(sha256_bytes(&serde_json::to_vec(&(
        config.endpoint.trim_end_matches('/'),
        &config.bucket,
        &config.prefix,
        &config.age_recipient,
    ))?))
}

pub(super) fn name(run_id: Uuid) -> String {
    format!("{run_id}.json")
}

pub(super) fn missing(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    })
}

pub(in crate::backup::scheduled) fn completed_for(
    journal: &Journal,
    receipt: &RunReceipt,
) -> anyhow::Result<bool> {
    let directory = journal.state.path.join("offsite/completed");
    match std::fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let directory = PrivateDirectory::prepare(&directory)?;
    let bytes = match directory.read(&name(receipt.run_id)) {
        Ok(bytes) => bytes,
        Err(error) if missing(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    let completed: CompletedRun = serde_json::from_slice(&bytes).context("本地副本完成事实无效")?;
    Ok(completed.format == COMPLETED_FORMAT
        && completed.run_id == receipt.run_id
        && completed.output_dir == receipt.output_dir
        && completed.database_name_sha256 == receipt.database_name_sha256
        && receipt.archive.as_ref().is_some_and(|archive| {
            archive.verified && archive.manifest_sha256 == completed.manifest_sha256
        })
        && completed.offsite.enabled
        && completed.offsite.daily.is_some())
}
