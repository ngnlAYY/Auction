//! archive、manifest、完成记录按顺序上传；每个远端副作用之前和之后都保存检查点。

use anyhow::Context;
use auction_integrations::R2BackupStore;
use chrono::Utc;

use super::super::{filesystem::PrivateDirectory, journal::Journal, model::RunReceipt};
use super::{
    encryption,
    model::{Pending, ReplicaProgress, ReplicaRecord, ScopeReplica, UPLOAD_FORMAT, UploadRecord},
    state::State,
};

pub(super) struct Transfer<'a> {
    pub store: &'a R2BackupStore,
    pub state: &'a State,
    /// 已封存密文的私有暂存目录（archive 与 manifest）。
    pub staging: &'a PrivateDirectory,
    /// 备份输出目录；范围密文与 sidecar 已在这里封存，不复制到暂存目录。
    pub output: &'a PrivateDirectory,
    pub current: &'a mut RunReceipt,
    pub journal: &'a Journal,
}

impl Transfer<'_> {
    pub(super) async fn upload(
        &mut self,
        item: &mut Pending,
        base: &str,
        monthly: bool,
    ) -> anyhow::Result<ReplicaRecord> {
        let Self { store, state, staging, output, current, journal } = self;
        if let Some(completed) = completed(progress(item, monthly)) {
            return Ok(completed);
        }
        let archive = item.archive_cipher.clone().context("缺少封存 archive 密文")?;
        let manifest = item.manifest_cipher.clone().context("缺少封存 manifest 密文")?;
        if progress(item, monthly).archive.is_none() {
            encryption::validate(staging, &archive).await?;
            let object = store
                .upload_verified(
                    &format!("{base}/{}", archive.file_name),
                    staging.open(&archive.file_name)?,
                    archive.identity.bytes,
                    &archive.sha256,
                )
                .await?;
            progress_mut(item, monthly).archive = Some(object);
            checkpoint(state, item, current, journal)?;
        }
        if progress(item, monthly).manifest.is_none() {
            encryption::validate(staging, &manifest).await?;
            let object = store
                .upload_verified(
                    &format!("{base}/{}", manifest.file_name),
                    staging.open(&manifest.file_name)?,
                    manifest.identity.bytes,
                    &manifest.sha256,
                )
                .await?;
            progress_mut(item, monthly).manifest = Some(object);
            checkpoint(state, item, current, journal)?;
        }
        if item.scope_bundle.is_some()
            && progress(item, monthly).scope.as_ref().and_then(ScopeReplica::complete).is_none()
        {
            upload_scope(store, output, item, monthly, base).await?;
            checkpoint(state, item, current, journal)?;
        }
        if progress(item, monthly).record.is_none() {
            let result = progress(item, monthly);
            let record = UploadRecord {
                format: UPLOAD_FORMAT.to_owned(),
                run_id: item.run_id,
                created_at: item.archive.created_at,
                archive: result.archive.clone().context("缺少 archive 副本")?,
                manifest: result.manifest.clone().context("缺少 manifest 副本")?,
                plaintext_manifest_sha256: item.archive.manifest_sha256.clone(),
                age_version: item.age_version.clone(),
                verified_at: Utc::now(),
                scope: result.scope.clone(),
            };
            progress_mut(item, monthly).record = Some(record);
            // 先固定完成记录的时间和字节，保证响应丢失后仍可原样对账。
            checkpoint(state, item, current, journal)?;
        }
        if progress(item, monthly).completion.is_none() {
            let payload = serde_json::to_vec_pretty(
                progress(item, monthly).record.as_ref().context("缺少完成记录")?,
            )?;
            let object = store.upload_record(&format!("{base}/completed.json"), payload).await?;
            progress_mut(item, monthly).completion = Some(object);
            checkpoint(state, item, current, journal)?;
        }
        let record = completed(progress(item, monthly)).context("副本尚未完成校验")?;
        // 本次备份带范围证据时，daily/monthly 都必须在完成记录里带上两个范围对象。
        if item.scope_bundle.is_some() && record.scope.is_none() {
            anyhow::bail!("范围副本尚未完成上传");
        }
        Ok(record)
    }
}

pub(super) fn completed(progress: &ReplicaProgress) -> Option<ReplicaRecord> {
    Some(ReplicaRecord {
        archive: progress.archive.clone()?,
        manifest: progress.manifest.clone()?,
        completion: progress.completion.clone()?,
        verified_at: progress.record.as_ref()?.verified_at,
        scope: progress.scope.clone().filter(|scope| scope.complete().is_some()),
    })
}

/// 先上传范围密文，再上传只含摘要的 sidecar；两者都只从输出目录读取原文件。
async fn upload_scope(
    store: &R2BackupStore,
    output: &PrivateDirectory,
    item: &mut Pending,
    monthly: bool,
    base: &str,
) -> anyhow::Result<()> {
    let bundle = item.scope_bundle.clone().context("缺少范围密文")?;
    let sidecar = item.scope_sidecar.clone().context("缺少范围 sidecar")?;
    if progress(item, monthly).scope.as_ref().and_then(|scope| scope.bundle.as_ref()).is_none() {
        encryption::validate(output, &bundle).await?;
        let object = store
            .upload_verified(
                &format!("{base}/{}", bundle.file_name),
                output.open(&bundle.file_name)?,
                bundle.identity.bytes,
                &bundle.sha256,
            )
            .await?;
        progress_mut(item, monthly).scope.get_or_insert_with(ScopeReplica::default).bundle =
            Some(object);
    }
    if progress(item, monthly).scope.as_ref().and_then(|scope| scope.sidecar.as_ref()).is_none() {
        encryption::validate(output, &sidecar).await?;
        let object = store
            .upload_verified(
                &format!("{base}/{}", sidecar.file_name),
                output.open(&sidecar.file_name)?,
                sidecar.identity.bytes,
                &sidecar.sha256,
            )
            .await?;
        progress_mut(item, monthly).scope.get_or_insert_with(ScopeReplica::default).sidecar =
            Some(object);
    }
    Ok(())
}

pub(super) fn checkpoint(
    state: &State,
    item: &Pending,
    current: &mut RunReceipt,
    journal: &Journal,
) -> anyhow::Result<()> {
    state.save(item)?;
    if item.run_id == current.run_id {
        current.offsite.daily = completed(&item.daily);
        current.offsite.monthly = completed(&item.monthly);
        current.offsite.monthly_written = current.offsite.monthly.is_some();
        current.offsite.pending_objects = [
            item.daily.archive.as_ref(),
            item.daily.manifest.as_ref(),
            item.daily.scope.as_ref().and_then(|scope| scope.bundle.as_ref()),
            item.daily.scope.as_ref().and_then(|scope| scope.sidecar.as_ref()),
            item.monthly.archive.as_ref(),
            item.monthly.manifest.as_ref(),
            item.monthly.scope.as_ref().and_then(|scope| scope.bundle.as_ref()),
            item.monthly.scope.as_ref().and_then(|scope| scope.sidecar.as_ref()),
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
        journal.checkpoint(current)?;
    }
    Ok(())
}

fn progress(item: &Pending, monthly: bool) -> &ReplicaProgress {
    if monthly { &item.monthly } else { &item.daily }
}

fn progress_mut(item: &mut Pending, monthly: bool) -> &mut ReplicaProgress {
    if monthly { &mut item.monthly } else { &mut item.daily }
}
