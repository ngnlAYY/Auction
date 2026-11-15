//! 内部一致性巡检入口；只连接本地配置指定的数据库，不启动 Worker 或外部账号能力。

use std::path::Path;

use anyhow::Context;

use crate::app::print_json;
use crate::config_dir::ConfigDir;
use crate::support::files;

pub(super) async fn run(
    config_dir: &ConfigDir,
    database: &str,
    output: &Path,
) -> anyhow::Result<()> {
    files::require_private_target(output)?;
    let config = super::confirmed_worker(config_dir, database)?;
    let report = super::with_store(
        &config,
        auction_storage::DatabaseConnectionPurpose::CliInventory,
        async |store| store.inspect_consistency(database).await.context("执行内部一致性巡检失败"),
    )
    .await?;
    files::write_private(output, &serde_json::to_vec_pretty(&report)?)?;
    print_json(&serde_json::json!({
        "report_id": report.report_id,
        "output": output,
        "database": report.database_name,
        "complete": report.complete,
        "consistent": report.consistent,
        "checks_completed": report.checks.len(),
        "failure_code": report.failure_code,
        "asset_actions_authorized": false,
    }))?;
    anyhow::ensure!(report.complete, "巡检未完成；报告已追加保存，详见 report_id 与 output");
    anyhow::ensure!(
        report.consistent,
        "巡检发现内部差异；报告已追加保存，详见 report_id 与 output"
    );
    Ok(())
}
