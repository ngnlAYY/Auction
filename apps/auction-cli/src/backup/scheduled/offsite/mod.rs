//! 本地 verify 之后的异机副本编排；明文、原密文与待上传证据在远端失败时全部保留。

use anyhow::Context;
use auction_config::AppConfig;
use auction_integrations::R2BackupStore;

use super::{
    filesystem::PrivateDirectory,
    journal::Journal,
    model::{Failure, RunReceipt, Stage},
    pipeline,
};
use crate::app::progress;

mod encryption;
mod model;
mod monthly;
mod processing;
mod queue;
mod replica;
mod staging;
mod state;

pub(super) use model::OffsiteRecord;
pub(super) use staging::{DIRECTORY as STAGING_DIRECTORY, reservation};
pub(super) use state::completed_for;

use model::{PENDING_FORMAT, Pending, ReplicaProgress};
use processing::Processor;
use state::State;

pub(super) struct Prepared {
    store: R2BackupStore,
    age_version: String,
    destination: String,
}

pub(super) async fn preflight(
    journal: &Journal,
    config: &AppConfig,
    receipt: &mut RunReceipt,
) -> anyhow::Result<Option<Prepared>> {
    if !config.backup.offsite.enabled {
        return Ok(None);
    }
    pipeline::stage(journal, receipt, Stage::OffsitePreflight)?;
    let age_version = encryption::preflight().await?;
    let offsite = &config.backup.offsite;
    let store = R2BackupStore::new(
        &offsite.endpoint,
        &offsite.bucket,
        config.backup_r2_access_key_id().context("缺少 R2 access key")?,
        config.backup_r2_secret_access_key().context("缺少 R2 secret")?,
    )?;
    receipt.offsite.age_version = Some(age_version.clone());
    Ok(Some(Prepared { store, age_version, destination: state::destination(offsite)? }))
}

pub(super) async fn after_verify(
    journal: &Journal,
    config: &AppConfig,
    output: &PrivateDirectory,
    receipt: &mut RunReceipt,
    prepared: Option<&Prepared>,
) -> anyhow::Result<()> {
    let Some(prepared) = prepared else {
        return Ok(());
    };
    execute(journal, config, output, receipt, prepared).await
}

async fn execute(
    journal: &Journal,
    config: &AppConfig,
    output: &PrivateDirectory,
    receipt: &mut RunReceipt,
    prepared: &Prepared,
) -> anyhow::Result<()> {
    pipeline::stage(journal, receipt, Stage::OffsiteEncrypt)?;
    let state = State::open(journal)?;
    let mut item = Pending {
        format: PENDING_FORMAT.to_owned(),
        run_id: receipt.run_id,
        started_at: receipt.started_at,
        output_dir: receipt.output_dir.clone(),
        database_name_sha256: receipt.database_name_sha256.clone(),
        destination_sha256: prepared.destination.clone(),
        archive: receipt.archive.clone().context("缺少已校验备份")?,
        age_version: prepared.age_version.clone(),
        archive_cipher: None,
        manifest_cipher: None,
        daily: ReplicaProgress::default(),
        write_monthly: None,
        monthly: ReplicaProgress::default(),
    };
    // 在加密前持久化任务身份，崩溃后能认领已封存密文；UUID 碰撞也不能覆盖旧任务。
    state.pending.write(&state::name(item.run_id), &serde_json::to_vec_pretty(&item)?, false)?;
    // 有限总预算优先取得最新恢复点，再尝试旧项；一个项目失败只记录该项目，不饿死后续项目。
    let processor = Processor { journal, config, output, prepared, state: &state };
    processor.attempt(&mut item, receipt).await?;
    pipeline::stage(journal, receipt, Stage::OffsiteRetry)?;
    for mut pending in state.queue(receipt, &prepared.destination, journal).await? {
        processor.attempt(&mut pending, receipt).await?;
    }
    if receipt.failure.is_some() {
        anyhow::bail!("异机副本队列存在失败项；首个失败及逐项结果见回执");
    }
    Ok(())
}

fn record_failure(receipt: &mut RunReceipt, run_id: uuid::Uuid, failure: Failure) {
    progress(format_args!(
        "backup offsite_run_id={run_id} stage={:?} error_code={}",
        failure.stage, failure.error_code
    ));
    receipt.offsite.failed(run_id, &failure);
    receipt.failure.get_or_insert(failure);
}

/// 总预算中止活动项目时仍记录该项；后来的 timeout 不覆盖更早的项目失败。
pub(super) fn timeout(receipt: &mut RunReceipt) {
    if let Some(run_id) = receipt.offsite.active_run_id.take() {
        record_failure(
            receipt,
            run_id,
            Failure { stage: receipt.stage, error_code: "timeout".to_owned() },
        );
    }
    receipt.fail("timeout");
}
