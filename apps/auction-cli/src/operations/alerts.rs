//! 主机侧运维告警报告：把磁盘水位、定时备份终态与 WAL 暂存积压写入 `backup`/`wal` 来源。
//!
//! 只读取已经发布的证据（`statvfs` 与终态回执/manifest），不执行备份、上传或迁移。写入走
//! `OperationsAlertStore::observe_host`：同一 `kind`/`scope` 命中同一 fingerprint，CLI 重试
//! 是 upsert，不会产生第二条 signal，也不会重置已排定的下一次通知时间；`backup` 与 `wal`
//! 各自独立观察，缺少一项证据不会关闭另一项来源的历史告警。

use std::time::Duration;

use anyhow::Context;
use auction_application::{
    OperationsAlertHostBackupFacts, OperationsAlertHostBackupOutcome, OperationsAlertHostBackupRun,
    OperationsAlertHostDisk, OperationsAlertHostObserveCommand, OperationsAlertHostWalFacts,
    OperationsAlertSource, OperationsAlertSourceObservation, OperationsAlertStore,
    evaluate_host_backup, evaluate_host_wal, lower_hex,
};
use auction_storage::DatabaseConnectionPurpose;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::app::{print_json, progress};
use crate::backup::{
    DiskUsage, TerminalOutcome, disk_usage, latest_terminal_run, staged_wal_facts,
};
use crate::cli::AlertCommand;
use crate::deployment::ConfigDir;

/// 单个来源的观察结果与固定告警码；告警码只说明证据缺失原因，不含路径或原始错误。
struct Observation {
    source: OperationsAlertSource,
    warnings: Vec<&'static str>,
    observation: OperationsAlertSourceObservation,
}

pub(crate) async fn run(config_dir: &ConfigDir, command: AlertCommand) -> anyhow::Result<()> {
    match command {
        AlertCommand::Report { backup_output_dir, backup_state_dir, wal_staging_dir } => {
            report(config_dir, &backup_output_dir, &backup_state_dir, wal_staging_dir.as_deref())
                .await
        }
    }
}

async fn report(
    config_dir: &ConfigDir,
    backup_output_dir: &std::path::Path,
    backup_state_dir: &std::path::Path,
    wal_staging_dir: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    require_absolute(backup_output_dir, "备份输出目录")?;
    require_absolute(backup_state_dir, "备份状态目录")?;
    if let Some(directory) = wal_staging_dir {
        require_absolute(directory, "WAL 暂存目录")?;
    }
    let (config, store) =
        super::worker_store(config_dir, DatabaseConnectionPurpose::CliOperations).await?;
    config.validate_backup().context("备份配置无效")?;
    let now = Utc::now();
    let mut observations =
        vec![backup_observation(&config, backup_output_dir, backup_state_dir, now)?];
    if let Some(directory) = wal_staging_dir {
        observations.push(wal_observation(&config, directory, now)?);
    }
    for observation in &observations {
        for warning in &observation.warnings {
            progress(format_args!(
                "alert_report source={} warning={warning}",
                observation.source.as_str()
            ));
        }
    }

    // 告警功能未启用时不写信号：主机侧报告没有 Worker 的扫描水位，无法为已停用的告警表
    // 承担通知，写入只会留下无人处理的活动行。
    if !store.health().await?.enabled {
        print_json(&Report {
            outcome: "skipped",
            reason: "operations_alert_disabled",
            now,
            batch_window_seconds: config.worker.operations_alert.batch_window_seconds,
            sources: Vec::new(),
        })?;
        store.postgres_pool().close().await;
        return Ok(());
    }

    let command = OperationsAlertHostObserveCommand {
        observations: observations
            .iter()
            .map(|observation| observation.observation.clone())
            .collect(),
        now,
        batch_window: Duration::from_secs(config.worker.operations_alert.batch_window_seconds),
    };
    store.observe_host(command).await?;
    let report = Report {
        outcome: "reported",
        reason: "",
        now,
        batch_window_seconds: config.worker.operations_alert.batch_window_seconds,
        sources: observations.iter().map(SourceReport::from).collect(),
    };
    print_json(&report)?;
    store.postgres_pool().close().await;
    Ok(())
}

fn backup_observation(
    config: &auction_config::AppConfig,
    output_dir: &std::path::Path,
    state_dir: &std::path::Path,
    now: DateTime<Utc>,
) -> anyhow::Result<Observation> {
    let mut warnings = Vec::new();
    let disk = match disk_usage(output_dir) {
        Ok(usage) => Some(usage),
        Err(error) => {
            progress(format_args!("alert_report backup_disk_unreadable: {error:#}"));
            warnings.push("backup_output_dir_unreadable");
            None
        }
    };
    let latest = match latest_terminal_run(state_dir) {
        Ok(Some(run)) => {
            let age_seconds = now.signed_duration_since(run.finished_at).num_seconds().max(0);
            Some(OperationsAlertHostBackupRun {
                outcome: match run.outcome {
                    TerminalOutcome::Completed => OperationsAlertHostBackupOutcome::Completed,
                    TerminalOutcome::Failed => OperationsAlertHostBackupOutcome::Failed,
                    TerminalOutcome::Interrupted => OperationsAlertHostBackupOutcome::Interrupted,
                },
                age_seconds,
            })
        }
        Ok(None) => {
            warnings.push("backup_receipts_missing");
            None
        }
        Err(error) => {
            progress(format_args!("alert_report backup_receipts_unreadable: {error:#}"));
            warnings.push("backup_receipts_unreadable");
            None
        }
    };
    let facts = OperationsAlertHostBackupFacts {
        disk: disk.map(|DiskUsage { total_bytes, available_bytes }| OperationsAlertHostDisk {
            total_bytes,
            available_bytes,
            min_free_bytes: config.backup.min_free_bytes,
            min_free_percent: config.backup.min_free_percent,
        }),
        latest_run: latest,
    };
    Ok(Observation {
        source: OperationsAlertSource::Backup,
        warnings,
        observation: evaluate_host_backup(&facts)?,
    })
}

fn wal_observation(
    config: &auction_config::AppConfig,
    staging_dir: &std::path::Path,
    now: DateTime<Utc>,
) -> anyhow::Result<Observation> {
    // 异机副本关闭时上传器不会删除 WAL 暂存，积压是正常保留而非故障；此时只解除历史告警。
    if !config.backup.offsite.enabled {
        return Ok(Observation {
            source: OperationsAlertSource::Wal,
            warnings: vec!["wal_offsite_disabled"],
            observation: evaluate_host_wal(&OperationsAlertHostWalFacts::default())?,
        });
    }
    let now_unix_seconds = u64::try_from(now.timestamp()).unwrap_or(0);
    let facts = match staged_wal_facts(staging_dir, now_unix_seconds) {
        Ok(facts) => facts,
        Err(error) => {
            progress(format_args!("alert_report wal_staging_unreadable: {error:#}"));
            return Ok(Observation {
                source: OperationsAlertSource::Wal,
                warnings: vec!["wal_staging_unreadable"],
                observation: OperationsAlertSourceObservation::unavailable(
                    OperationsAlertSource::Wal,
                )?,
            });
        }
    };
    let mut warnings = Vec::new();
    if facts.invalid_entries > 0 {
        warnings.push("wal_invalid_entries");
    }
    Ok(Observation {
        source: OperationsAlertSource::Wal,
        warnings,
        observation: evaluate_host_wal(&OperationsAlertHostWalFacts {
            staged_segments: facts.staged_segments,
            oldest_staged_age_seconds: facts.oldest_staged_age_seconds,
        })?,
    })
}

fn require_absolute(path: &std::path::Path, label: &str) -> anyhow::Result<()> {
    if !path.is_absolute() {
        anyhow::bail!("{label}必须是绝对路径：{}", path.display());
    }
    Ok(())
}

#[derive(Serialize)]
struct Report {
    outcome: &'static str,
    reason: &'static str,
    now: DateTime<Utc>,
    batch_window_seconds: u64,
    sources: Vec<SourceReport>,
}

#[derive(Serialize)]
struct SourceReport {
    source: &'static str,
    available: bool,
    warnings: Vec<&'static str>,
    signals: Vec<SignalReport>,
}

impl From<&Observation> for SourceReport {
    fn from(observation: &Observation) -> Self {
        Self {
            source: observation.source.as_str(),
            available: observation.observation.available,
            warnings: observation.warnings.clone(),
            signals: observation
                .observation
                .signals
                .iter()
                .filter_map(|signal| {
                    Some(SignalReport {
                        kind: signal.kind.as_str(),
                        scope: signal.scope.clone(),
                        count: signal.count,
                        age_seconds: signal.age_seconds,
                        fingerprint: lower_hex(&signal.fingerprint),
                        summary: signal.render_summary().ok()?,
                    })
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct SignalReport {
    kind: &'static str,
    scope: String,
    count: i64,
    age_seconds: i64,
    fingerprint: String,
    summary: String,
}
