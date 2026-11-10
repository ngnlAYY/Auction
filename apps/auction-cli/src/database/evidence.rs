//! 只读恢复预检与保留盘点；证据以不可覆盖的私有 JSON 文件保存，stdout 只输出摘要。

use std::path::Path;

use anyhow::Context;
use auction_storage::DatabaseStore;

use super::DatabaseTarget;
use crate::app::print_json;
use crate::deployment::ConfigDir;
use crate::support::files;

pub(super) async fn preflight(
    config_dir: &ConfigDir,
    database: &str,
    output: &Path,
) -> anyhow::Result<()> {
    files::require_private_target(output)?;
    let store =
        connect(config_dir, database, auction_storage::DatabaseConnectionPurpose::CliPreflight)
            .await?;
    let result = super::preflight(&store, database).await;
    store.postgres_pool().close().await;
    let preflight = result?;
    files::write_private(output, &serde_json::to_vec_pretty(&preflight)?)?;
    print_json(&serde_json::json!({
        "output": output,
        "database": preflight.database_name,
        "restore_preflight_passed": preflight.restore_preflight_passed,
        "active_other_sessions": preflight.active_other_sessions,
        "user_object_count": preflight.user_object_count,
    }))
}

pub(super) async fn inventory(
    config_dir: &ConfigDir,
    database: &str,
    output: &Path,
) -> anyhow::Result<()> {
    files::require_private_target(output)?;
    let store =
        connect(config_dir, database, auction_storage::DatabaseConnectionPurpose::CliInventory)
            .await?;
    let postgres = &store;
    let result = postgres.collect_retention_inventory(database).await;
    store.postgres_pool().close().await;
    let inventory = result.context("收集保留盘点失败")?;
    files::write_private(output, &serde_json::to_vec_pretty(&inventory)?)?;
    print_json(&serde_json::json!({
        "output": output,
        "database": inventory.database_name,
        "relation_count": inventory.relation_count,
        "policy_table_count": inventory.policy_table_count,
        "policy_sha256": inventory.policy_sha256,
        "migration_current": inventory.migration.current,
        "eligibility_authorized": inventory.eligibility_authorized,
    }))
}

pub(super) async fn retention_dry_run(
    config_dir: &ConfigDir,
    database: &str,
    output: &Path,
) -> anyhow::Result<()> {
    files::require_private_target(output)?;
    let config = config_dir.load_worker()?;
    let url = config.database_url().context("Worker 配置缺少数据库连接")?;
    DatabaseTarget::parse(url)?.require_database(database)?;
    let store =
        super::connect(&config, auction_storage::DatabaseConnectionPurpose::CliRetentionDryRun)
            .await?;
    let postgres = &store;
    let run_id = uuid::Uuid::new_v4();
    let result = postgres.collect_retention_dry_run(database, run_id).await;
    store.postgres_pool().close().await;
    let report = result.context("收集保留清理只读演练失败")?;

    let ordinary_days =
        auction_storage::retention_policy_rule_duration_days("mail_diagnostic_ordinary_age")
            .context("政策 v2 缺少一般诊断保留期限")?;
    let hold_days =
        auction_storage::retention_policy_rule_duration_days("mail_diagnostic_write_unknown_hold")
            .context("政策 v2 缺少写入未知证据保全期限")?;
    let seconds = |days: i64| -> anyhow::Result<u64> {
        Ok(u64::try_from(days).context("政策期限超出范围")?.saturating_mul(86_400))
    };
    let snapshot = auction_integrations::mooglemail::mail_diagnostic_retention_snapshot(
        &config.worker.mooglemail.diagnostics.directory,
        std::time::Duration::from_secs(seconds(ordinary_days)?),
        std::time::Duration::from_secs(seconds(hold_days)?),
        std::time::SystemTime::now(),
    )
    .map_err(|error| anyhow::anyhow!("只读统计加密邮件诊断文件失败：{}", error.as_str()))?;

    let filesystem = serde_json::json!({
        "dataset_family": "mail_diagnostic_files",
        "policy_state": "delete_after_age",
        "retention_kind": "filesystem",
        "directory_present": snapshot.directory_present,
        "ordinary_retention_days": ordinary_days,
        "write_unknown_hold_days": hold_days,
        "total_files": snapshot.total_files,
        "candidate_files": snapshot.ordinary_expired_files,
        "candidate_bytes": snapshot.ordinary_expired_bytes,
        "write_unknown_held_files": snapshot.write_unknown_held_files,
        "write_unknown_held_bytes": snapshot.write_unknown_held_bytes,
        "write_unknown_over_hold_window_files": snapshot.write_unknown_over_hold_window_files,
        "retained_files": snapshot.retained_files,
        "temporary_files": snapshot.temporary_files,
        "unexpected_entries": snapshot.unexpected_entries,
        "mutation_executed": false,
        "deletion_authorized": false,
    });
    let receipt = serde_json::json!({
        "format": "auction-retention-dry-run-receipt.v1",
        "run_id": run_id,
        "created_at": chrono::Utc::now(),
        "database_retention": &report,
        "filesystem_retention": filesystem,
        "totals": {
            "database_candidate_rows": report.candidate_rows,
            "database_retained_rows": report.retained_rows,
            "filesystem_candidate_files": snapshot.ordinary_expired_files,
            "candidate_units_are_distinct": true,
            "mutation_executed": false,
            "deletion_authorized": false,
        },
        "notes": [
            "只读回执：数据库候选与文件候选单位不同，不能相加；两者都不授权删除",
            "实际清理须另行批准，按表分批执行并重新演练"
        ],
    });
    files::write_private(output, &serde_json::to_vec_pretty(&receipt)?)?;
    print_json(&serde_json::json!({
        "output": output,
        "run_id": run_id,
        "database": report.database_name,
        "policy_version": report.policy_version,
        "policy_sha256": report.policy_sha256,
        "candidate_rows": report.candidate_rows,
        "retained_rows": report.retained_rows,
        "blocked_rows": report.families.iter().map(|family| family.blocked_rows).sum::<i64>(),
        "diagnostic_candidate_files": snapshot.ordinary_expired_files,
        "mutation_executed": report.mutation_executed,
        "deletion_authorized": report.deletion_authorized,
    }))
}

async fn connect(
    config_dir: &ConfigDir,
    database: &str,
    purpose: auction_storage::DatabaseConnectionPurpose,
) -> anyhow::Result<DatabaseStore> {
    let config = config_dir.load_worker()?;
    let url = config.database_url().context("Worker 配置缺少数据库连接")?;
    DatabaseTarget::parse(url)?.require_database(database)?;
    super::connect(&config, purpose).await
}
