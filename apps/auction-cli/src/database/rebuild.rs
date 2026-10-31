//! 破坏性重建：持锁预检 → 精确确认 → DROP/CREATE 同定义空库 →（可选）迁移。
//! 默认只读演练；一旦开始 DROP，不论成败都发布记录已知破坏性状态的回执。

use std::path::Path;

use anyhow::Context;
use auction_storage::{PostgresDatabaseRebuildGuard, PostgresRebuildError};
use chrono::Utc;
use serde_json::{Value, json};

use super::DatabaseTarget;
use crate::app::{print_json, progress};
use crate::cli::{MaintenanceArgs, RebuildModeArg};
use crate::deployment::ConfigDir;
use crate::support::{files, text};

const RECEIPT_FORMAT: &str = "auction-postgres-rebuild.v1";

pub(super) struct Options<'a> {
    pub(super) config_dir: &'a ConfigDir,
    pub(super) database: &'a str,
    pub(super) maintenance_secrets: &'a Path,
    pub(super) mode: RebuildModeArg,
    pub(super) backup_manifest: Option<&'a Path>,
    pub(super) ephemeral: bool,
    pub(super) execute: bool,
    pub(super) confirm: Option<&'a str>,
    pub(super) receipt: Option<&'a Path>,
    pub(super) maintenance: &'a MaintenanceArgs,
}

pub(super) async fn run(options: Options<'_>) -> anyhow::Result<()> {
    if options.backup_manifest.is_none() && !options.ephemeral {
        anyhow::bail!("重建需要 --backup-manifest（已校验的恢复点）或明确的 --ephemeral");
    }
    if options.mode == RebuildModeArg::EmptyForRestore && options.backup_manifest.is_none() {
        anyhow::bail!("empty-for-restore 必须提供随后要恢复的 --backup-manifest");
    }
    let mode = match options.mode {
        RebuildModeArg::EmptyForRestore => "empty-for-restore",
        RebuildModeArg::FreshSchema => "fresh-schema",
    };
    if options.execute {
        let expected = if options.ephemeral {
            format!("EPHEMERAL {}", options.database)
        } else {
            format!("REBUILD {}", options.database)
        };
        text::confirmation(options.confirm.unwrap_or_default(), &expected)?;
        files::require_private_target(options.receipt.context("实际重建必须提供 --receipt")?)?;
    }

    let guard =
        crate::service::acquire_maintenance(options.config_dir, options.maintenance).await?;
    let config = options.config_dir.load_worker()?;
    let application_url = config.database_url().context("Worker 配置缺少数据库连接")?;
    let application = DatabaseTarget::parse(application_url)?;
    application.require_database(options.database)?;
    let secrets = auction_config::load_postgres_maintenance_secrets(options.maintenance_secrets)
        .context("读取维护凭据失败")?;
    let maintenance_target = DatabaseTarget::parse(secrets.database_url())?;
    application.require_same_endpoint(&maintenance_target)?;

    let backup = match options.backup_manifest {
        Some(manifest) => Some(crate::backup::verify_pair(manifest, options.database).await?),
        None => None,
    };
    if options.mode == RebuildModeArg::EmptyForRestore
        && let Some(backup) = &backup
    {
        backup.require_supported_schema()?;
    }
    let mut rebuild =
        PostgresDatabaseRebuildGuard::connect(secrets.database_url(), options.database)
            .await
            .context("重建预检未通过")?;
    let preflight = serde_json::to_value(rebuild.preflight())?;
    let backup_summary = backup.as_ref().map(|backup| {
        json!({
            "manifest_sha256": backup.manifest_sha256,
            "archive_sha256": backup.manifest.archive_sha256,
            "schema_version": backup.manifest.schema_version,
        })
    });
    if !options.execute {
        return print_json(&json!({
            "outcome": "dry_run", "guard": guard.mode, "mode": mode,
            "preflight": preflight, "backup": backup_summary,
        }));
    }

    // DROP 前复核恢复点，拒绝预检期间被替换或改写的 archive/manifest。
    if let (Some(path), Some(verified)) = (options.backup_manifest, &backup) {
        let current = crate::backup::verify_pair(path, options.database).await?;
        if current.manifest_sha256 != verified.manifest_sha256
            || current.manifest.archive_sha256 != verified.manifest.archive_sha256
        {
            anyhow::bail!("备份 archive 或 manifest 在预检期间发生变化，已停止重建");
        }
    }
    let receipt_path = options.receipt.context("实际重建必须提供 --receipt")?;
    progress(format_args!("开始 DROP/CREATE {}", options.database));
    let mut receipt = json!({
        "format": RECEIPT_FORMAT,
        "started_at": Utc::now(),
        "database": options.database,
        "mode": mode,
        "ephemeral": options.ephemeral,
        "backup": backup_summary,
        "preflight": preflight,
    });
    let outcome = match rebuild.recreate_empty().await {
        Ok(result) => {
            receipt["rebuild"] = serde_json::to_value(&result)?;
            if options.mode == RebuildModeArg::FreshSchema {
                match super::apply_migrations(application_url).await {
                    Ok(report) => {
                        receipt["migration"] = serde_json::to_value(report)?;
                        Ok(())
                    }
                    Err(error) => {
                        receipt["failure"] = json!({
                            "stage": "migrate", "database_dropped": true,
                            "database_recreated": true, "error": format!("{error:#}"),
                        });
                        Err(error.context("空库已重建，但迁移失败；保持停机并人工核对"))
                    }
                }
            } else {
                Ok(())
            }
        }
        Err(error) => {
            receipt["failure"] = failure(&error);
            Err(anyhow::Error::new(error)
                .context("重建失败；未知结果不能推断为未执行，禁止盲目重试"))
        }
    };
    receipt["finished_at"] = json!(Utc::now());
    receipt["outcome"] = json!(if outcome.is_ok() { "completed" } else { "failed" });
    files::write_private(receipt_path, &serde_json::to_vec_pretty(&receipt)?)
        .context("重建已执行，但回执写入失败")?;
    outcome?;
    print_json(&json!({
        "outcome": "completed", "guard": guard.mode, "mode": mode, "receipt": receipt_path,
        "next_step": if options.mode == RebuildModeArg::EmptyForRestore {
            "auction-cli backup restore"
        } else {
            "auction-cli service start"
        },
    }))
}

fn failure(error: &PostgresRebuildError) -> Value {
    match error.mutation_state() {
        Some((stage, dropped, recreated)) => json!({
            "stage": stage, "database_dropped": dropped, "database_recreated": recreated,
            "error": error.to_string(),
        }),
        None => {
            json!({ "stage": "preflight", "database_dropped": false, "error": error.to_string() })
        }
    }
}
