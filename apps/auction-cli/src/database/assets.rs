//! 资产报告 CLI 入口：只读最近报告，或仅从数据库投影追加一份偿付能力报告。
//!
//! 不读取 Armory、不启动 Worker、不改变余额或装备；`reconcile-custody` 依赖外部 Armory 观察，
//! 由 Worker 周期执行，CLI 不伪造观察结果。

use std::path::Path;

use anyhow::Context;
use auction_application::reconciliation::AssetReconciliationStore;

use crate::app::print_json;
use crate::config_dir::ConfigDir;
use crate::support::files;

pub(super) async fn latest(
    config_dir: &ConfigDir,
    database: &str,
    output: &Path,
) -> anyhow::Result<()> {
    files::require_private_target(output)?;
    let config = super::confirmed_worker(config_dir, database)?;
    let reports = super::with_store(
        &config,
        auction_storage::DatabaseConnectionPurpose::CliInventory,
        async |store| store.latest_asset_reports().await.context("读取资产报告失败"),
    )
    .await?;
    files::write_private(output, &serde_json::to_vec_pretty(&reports)?)?;
    print_json(&serde_json::json!({
        "output": output,
        "database": database,
        "custody_report": reports.custody.as_ref().map(|report| serde_json::json!({
            "report_id": report.report_id,
            "completed_at": report.completed_at,
            "complete": report.complete,
            "consistent": report.consistent,
            "failure_code": report.failure_code,
        })),
        "solvency_report": reports.solvency.as_ref().map(|report| serde_json::json!({
            "report_id": report.report_id,
            "completed_at": report.completed_at,
            "complete": report.complete,
            "solvent": report.solvent,
            "failure_code": report.failure_code,
        })),
        "asset_actions_authorized": false,
    }))?;
    Ok(())
}

pub(super) async fn report_solvency(
    config_dir: &ConfigDir,
    database: &str,
    output: &Path,
) -> anyhow::Result<()> {
    files::require_private_target(output)?;
    let config = super::confirmed_worker(config_dir, database)?;
    let report = super::with_store(
        &config,
        auction_storage::DatabaseConnectionPurpose::CliInventory,
        async |store| {
            store
                .report_solvency(config.worker.recovery.evidence_stale_after_seconds)
                .await
                .context("生成偿付能力报告失败")
        },
    )
    .await?;
    files::write_private(output, &serde_json::to_vec_pretty(&report)?)?;
    print_json(&serde_json::json!({
        "report_id": report.report_id,
        "output": output,
        "database": report.database_name,
        "complete": report.complete,
        "solvent": report.solvent,
        "worlds": report.worlds.iter().map(|world| serde_json::json!({
            "world": world.world,
            "required_credits": world.required_credits,
            "balance_credits": world.balance_credits,
            "verdict": world.verdict,
            "shortfall_credits": world.shortfall_credits,
        })).collect::<Vec<_>>(),
        "failure_code": report.failure_code,
        "asset_actions_authorized": false,
    }))?;
    anyhow::ensure!(
        report.complete,
        "偿付能力报告未完成；报告已追加保存，详见 report_id 与 output"
    );
    anyhow::ensure!(report.solvent, "偿付能力不足或余额未知；报告已追加保存，且不授权任何资金动作");
    Ok(())
}
