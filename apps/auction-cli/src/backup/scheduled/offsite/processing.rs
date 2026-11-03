//! 单个副本任务的执行和失败隔离；已知失败先记录，其他项目继续，终态由外层统一收口。

use anyhow::Context;
use auction_config::AppConfig;
use auction_integrations::BackupStoreError;

use super::super::{
    filesystem::PrivateDirectory,
    journal::Journal,
    model::{Failure, RunReceipt, Stage},
    pipeline,
};
use super::{
    Prepared,
    model::{COMPLETED_FORMAT, CompletedRun, OffsiteRecord, Pending},
    monthly, record_failure, replica, staging,
    state::State,
};
use crate::app::progress;

pub(super) struct Processor<'a> {
    pub journal: &'a Journal,
    pub config: &'a AppConfig,
    pub output: &'a PrivateDirectory,
    pub prepared: &'a Prepared,
    pub state: &'a State,
}

impl Processor<'_> {
    pub(super) async fn attempt(
        &self,
        item: &mut Pending,
        receipt: &mut RunReceipt,
    ) -> anyhow::Result<()> {
        receipt.offsite.active_run_id = Some(item.run_id);
        let retry = item.run_id != receipt.run_id;
        progress(format_args!("backup offsite_run_id={} retry={retry}", item.run_id));
        let result = self.process(item, receipt).await;
        receipt.offsite.active_run_id = None;
        if let Err(error) = result {
            let failure = if let Some(failure) = error.downcast_ref::<Failure>() {
                failure.clone()
            } else {
                let code = error
                    .downcast_ref::<BackupStoreError>()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| receipt.stage.error_code().to_owned());
                Failure { stage: receipt.stage, error_code: code }
            };
            record_failure(receipt, item.run_id, failure);
        } else if retry {
            receipt.offsite.retried_count = receipt.offsite.retried_count.saturating_add(1);
            if receipt.offsite.retried_runs.len() < 256 {
                receipt.offsite.retried_runs.push(item.run_id);
            }
        } else {
            receipt.offsite.pending_objects.clear();
        }
        // outcome 仍是 running，使强杀后能按正常残留记录收口；这里只持久化已知首错和逐项结果。
        // 状态目录已无法写入时不能再安全推进远端副作用，由外层以非零终态收口。
        self.journal.checkpoint(receipt)
    }

    async fn process(&self, item: &mut Pending, receipt: &mut RunReceipt) -> anyhow::Result<()> {
        pipeline::stage(self.journal, receipt, Stage::OffsiteEncrypt)?;
        let staging =
            staging::seal(self.state, item, self.output, self.config, receipt, self.journal)
                .await?;
        pipeline::stage(self.journal, receipt, Stage::OffsiteDaily)?;
        let daily = format!(
            "{}/daily/{}/{}",
            self.config.backup.offsite.prefix,
            item.archive.created_at.format("%Y/%m/%d"),
            item.run_id
        );
        replica::Transfer {
            store: &self.prepared.store,
            state: self.state,
            staging: &staging,
            current: receipt,
            journal: self.journal,
        }
        .upload(item, &daily, false)
        .await?;
        pipeline::stage(self.journal, receipt, Stage::OffsiteMonthly)?;
        if item.write_monthly != Some(false) && item.monthly.completion.is_none() {
            // 本次新副本优先后，旧任务的未完成月选择也要重新确认，不能多发布一份月完成记录。
            item.write_monthly = Some(
                monthly::should_write(
                    &self.prepared.store,
                    self.state,
                    item,
                    &self.config.backup.offsite.prefix,
                )
                .await?,
            );
            replica::checkpoint(self.state, item, receipt, self.journal)?;
        }
        if item.write_monthly == Some(true) {
            let monthly = format!(
                "{}/monthly/{}/{}",
                self.config.backup.offsite.prefix,
                item.archive.created_at.format("%Y/%m"),
                item.run_id
            );
            let completed = replica::Transfer {
                store: &self.prepared.store,
                state: self.state,
                staging: &staging,
                current: receipt,
                journal: self.journal,
            }
            .upload(item, &monthly, true)
            .await?;
            monthly::save(self.state, item, completed.completion)?;
        }
        pipeline::stage(self.journal, receipt, Stage::OffsiteFinalize)?;
        self.finalize(item, &staging)
    }

    fn finalize(&self, item: &Pending, staging: &PrivateDirectory) -> anyhow::Result<()> {
        let offsite = OffsiteRecord {
            enabled: true,
            age_version: Some(item.age_version.clone()),
            daily: Some(replica::completed(&item.daily).context("日副本未完成")?),
            monthly: replica::completed(&item.monthly),
            monthly_written: item.write_monthly == Some(true),
            ..Default::default()
        };
        self.state.finish(&CompletedRun {
            format: COMPLETED_FORMAT.to_owned(),
            run_id: item.run_id,
            output_dir: item.output_dir.clone(),
            database_name_sha256: item.database_name_sha256.clone(),
            manifest_sha256: item.archive.manifest_sha256.clone(),
            offsite,
        })?;
        staging::remove(self.output, staging, item)?;
        self.state.remove_pending(item.run_id)
    }
}
