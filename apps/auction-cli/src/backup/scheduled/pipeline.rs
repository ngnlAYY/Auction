//! 有总预算的备份阶段；所有错误只按阶段分类，底层连接与外部错误链不进入回执和日志。

use anyhow::Context;
use auction_config::AppConfig;

use super::{
    completion, disk,
    filesystem::{FileIdentity, PrivateDirectory},
    journal::Journal,
    model::{ArchiveRecord, RunReceipt, Stage},
    offsite, retention,
};
use crate::{
    app::progress,
    backup::{create::create_backup, manifest::sha256_bytes, verify_pair},
};

pub(super) async fn execute(
    journal: &Journal,
    config: &AppConfig,
    database: &str,
    receipt: &mut RunReceipt,
) -> anyhow::Result<()> {
    stage(journal, receipt, Stage::PrepareOutput)?;
    if receipt.output_dir.starts_with(&journal.state.path)
        || journal.state.path.starts_with(&receipt.output_dir)
    {
        anyhow::bail!("output-dir 与 state-dir 必须是不重叠的独立目录");
    }
    let output = PrivateDirectory::prepare(&receipt.output_dir)?;
    stage(journal, receipt, Stage::OutputLock)?;
    let _output_lock =
        output.try_lock("scheduled.lock")?.context("输出目录已有定时备份正在运行")?;
    stage(journal, receipt, Stage::DiskBefore)?;
    check_disk(&output, config, receipt, true)?;
    let offsite = offsite::preflight(journal, config, receipt).await?;
    stage(journal, receipt, Stage::Create)?;
    let archive_name = receipt.archive_name();
    let manifest_name = format!("{archive_name}.manifest");
    let manifest = create_backup(config, database, &output.path.join(&archive_name)).await?;
    let manifest_bytes = output.read(&manifest_name)?;
    receipt.archive = Some(ArchiveRecord {
        archive_file_name: archive_name.clone(),
        manifest_file_name: manifest_name.clone(),
        archive_sha256: manifest.archive_sha256,
        manifest_sha256: sha256_bytes(&manifest_bytes),
        schema_version: manifest.schema_version,
        created_at: manifest.created_at,
        archive_identity: FileIdentity::of(&output.open(&archive_name)?.metadata()?),
        manifest_identity: FileIdentity::of(&output.open(&manifest_name)?.metadata()?),
        verified: false,
    });
    stage(journal, receipt, Stage::DiskAfter)?;
    check_disk(&output, config, receipt, false)?;
    stage(journal, receipt, Stage::Verify)?;
    let verified = verify_pair(&output.path.join(&manifest_name), database).await?;
    let archive = receipt.archive.as_mut().context("备份结果缺少 archive")?;
    if verified.manifest_sha256 != archive.manifest_sha256
        || verified.manifest.archive_sha256 != archive.archive_sha256
        || FileIdentity::of(&output.open(&archive_name)?.metadata()?) != archive.archive_identity
        || FileIdentity::of(&output.open(&manifest_name)?.metadata()?) != archive.manifest_identity
    {
        anyhow::bail!("备份在创建与校验之间变化");
    }
    archive.verified = true;
    journal.checkpoint(receipt)?;
    completion::after_verify(journal, config, &output, receipt, offsite.as_ref()).await?;
    stage(journal, receipt, Stage::Retention)?;
    retention::clean(journal, &output, receipt).await
}

pub(super) fn stage(
    journal: &Journal,
    receipt: &mut RunReceipt,
    stage: Stage,
) -> anyhow::Result<()> {
    receipt.enter_stage(stage);
    journal.checkpoint(receipt)?;
    progress(format_args!("backup run_id={} stage={stage:?}", receipt.run_id));
    Ok(())
}

fn check_disk(
    output: &PrivateDirectory,
    config: &AppConfig,
    receipt: &mut RunReceipt,
    before: bool,
) -> anyhow::Result<()> {
    let reserved = if !before && config.backup.offsite.enabled {
        offsite::reservation(receipt.archive.as_ref().context("缺少密文预算的备份大小")?)?
    } else {
        0
    };
    let snapshot = disk::inspect(output, &config.backup, reserved)?;
    let passed = snapshot.passed;
    if before {
        receipt.disk_before = Some(snapshot);
    } else {
        receipt.disk_after = Some(snapshot);
    }
    if !passed {
        receipt.fail("disk_low");
        anyhow::bail!("备份磁盘低于配置水位");
    }
    Ok(())
}
