//! 内部一致性巡检入口；只连接本地配置指定的数据库，不启动 Worker 或外部账号能力。

use std::path::Path;

use anyhow::Context;

use super::DatabaseTarget;
use crate::app::print_json;
use crate::config_dir::ConfigDir;
use crate::support::files;

pub(super) async fn run(
    config_dir: &ConfigDir,
    database: &str,
    output: &Path,
) -> anyhow::Result<()> {
    files::require_private_target(output)?;
    let config = config_dir.load_worker()?;
    let url = config.database_url().context("Worker 配置缺少数据库连接")?;
    DatabaseTarget::parse(url)?.require_database(database)?;
    let store =
        super::connect(&config, auction_storage::DatabaseConnectionPurpose::CliInventory).await?;
    let postgres = &store;
    let result = postgres.inspect_consistency(database).await;
    store.postgres_pool().close().await;
    let report = result.context("执行内部一致性巡检失败")?;
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
