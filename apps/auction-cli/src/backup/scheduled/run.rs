//! 定时备份入口与总预算收口；先持状态目录锁，再恢复旧记录和加载 Worker 配置。

use std::{path::Path, time::Duration};

use auction_config::AppConfig;
use chrono::Utc;
use tokio::time::Instant;

use super::{
    heartbeat,
    journal::Journal,
    model::{Outcome, RunReceipt, Stage},
    offsite, pipeline,
};
use crate::{
    app::{print_json, progress},
    config_dir::ConfigDir,
};

pub(in crate::backup) async fn run(
    config_dir: &ConfigDir,
    database: &str,
    output_dir: &Path,
    state_dir: &Path,
) -> anyhow::Result<()> {
    let started = Instant::now();
    // begin 先取非阻塞独占锁；未取得锁时不创建运行记录，也不写回执。
    let journal = Journal::begin(state_dir)?;
    let mut receipt = RunReceipt::new(database, output_dir);
    let mut config = None;
    receipt.enter_stage(Stage::Recover);
    match journal.recover(&mut receipt) {
        Ok(interrupted) => receipt.recovered_interrupted = u64::from(interrupted),
        Err(_) => receipt.fail("recovery_failed"),
    }
    if receipt.failure.is_none() {
        // Config 阶段先落运行记录；加载失败也能发布终态，不依赖 Public/Admin 配置。
        if load_config(config_dir, &journal, &mut receipt, &mut config).is_err() {
            receipt.fail("config_error");
        }
    } else {
        // 收口失败时保留旧 running.json，仅读取失败打卡配置，不进入备份或清理。
        config = config_dir.load_worker().ok();
    }
    if receipt.failure.is_none() {
        if let Some(config) = config.as_ref() {
            let deadline = started + Duration::from_secs(config.backup.timeout_seconds);
            let result = if Instant::now() >= deadline {
                Err(())
            } else {
                tokio::time::timeout_at(
                    deadline,
                    pipeline::execute(&journal, config, database, &mut receipt),
                )
                .await
                .map_err(|_| ())
            };
            match result {
                Ok(Ok(())) if Instant::now() <= deadline && receipt.failure.is_none() => {
                    receipt.outcome = Outcome::Completed
                }
                Ok(Ok(())) if receipt.failure.is_some() => receipt.outcome = Outcome::Failed,
                Ok(Ok(())) => offsite::timeout(&mut receipt),
                Ok(Err(_)) if receipt.failure.is_some() => receipt.outcome = Outcome::Failed,
                Ok(Err(_)) => {
                    let code = receipt.stage.error_code();
                    receipt.fail(code);
                }
                Err(_) => offsite::timeout(&mut receipt),
            }
        } else {
            receipt.fail("config_error");
        }
    }
    receipt.finished_at = Some(Utc::now());
    receipt.record_elapsed();
    if receipt.outcome == Outcome::Completed {
        receipt.stage = Stage::Receipt;
    }
    receipt.heartbeat_configured = heartbeat::address(config.as_ref(), receipt.outcome).is_some();
    if journal.finish(&receipt).is_err() {
        receipt.stage = Stage::Receipt;
        receipt.fail("receipt_failed");
        // 终态存储失败仍尝试一次失败打卡；运行记录保持原样供下次收口。
        heartbeat::send(&receipt, config.as_ref()).await;
        anyhow::bail!("定时备份失败：receipt_failed；请核对运行记录和状态目录");
    }
    heartbeat::send(&receipt, config.as_ref()).await;
    progress(format_args!(
        "backup run_id={} outcome={:?} stage={:?} error_code={:?}",
        receipt.run_id,
        receipt.outcome,
        receipt.stage,
        receipt.failure.as_ref().map(|failure| failure.error_code.as_str())
    ));
    print_json(&serde_json::json!({
        "outcome": receipt.outcome, "run_id": receipt.run_id,
        "receipt": journal.receipts.path.join(receipt.receipt_name()),
        "failure": receipt.failure,
    }))?;
    if receipt.outcome != Outcome::Completed {
        anyhow::bail!("定时备份失败；请按回执的 stage/error_code 排查");
    }
    Ok(())
}

fn load_config(
    config_dir: &ConfigDir,
    journal: &Journal,
    receipt: &mut RunReceipt,
    available: &mut Option<AppConfig>,
) -> anyhow::Result<()> {
    pipeline::stage(journal, receipt, Stage::Config)?;
    let config = config_dir.load_worker()?;
    receipt.heartbeat_configured = config.backup_heartbeat_failure_url().is_some();
    *available = Some(config);
    if let Some(config) = available.as_ref() {
        config.validate_backup()?;
        receipt.policy = Some((&config.backup).into());
        receipt.offsite.enabled = config.backup.offsite.enabled;
    }
    journal.checkpoint(receipt)
}
