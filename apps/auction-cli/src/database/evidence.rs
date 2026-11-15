//! 只读恢复预检与保留盘点；证据以不可覆盖的私有 JSON 文件保存，stdout 只输出摘要。

use std::path::Path;

use anyhow::Context;

use crate::app::print_json;
use crate::config_dir::ConfigDir;
use crate::support::files;

pub(super) async fn preflight(
    config_dir: &ConfigDir,
    database: &str,
    output: &Path,
) -> anyhow::Result<()> {
    files::require_private_target(output)?;
    let config = super::confirmed_worker(config_dir, database)?;
    let preflight = super::with_store(
        &config,
        auction_storage::DatabaseConnectionPurpose::CliPreflight,
        async |store| super::preflight(store, database).await,
    )
    .await?;
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
    let config = super::confirmed_worker(config_dir, database)?;
    let inventory = super::with_store(
        &config,
        auction_storage::DatabaseConnectionPurpose::CliInventory,
        async |store| store.collect_retention_inventory(database).await.context("收集保留盘点失败"),
    )
    .await?;
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
    let config = super::confirmed_worker(config_dir, database)?;
    let run_id = uuid::Uuid::new_v4();
    let report = super::with_store(
        &config,
        auction_storage::DatabaseConnectionPurpose::CliRetentionDryRun,
        async |store| {
            store
                .collect_retention_dry_run(database, run_id)
                .await
                .context("收集保留清理只读演练失败")
        },
    )
    .await?;

    let (ordinary_days, hold_days) = super::mail_diagnostic_windows()?;
    let snapshot = auction_integrations::mooglemail::mail_diagnostic_retention_snapshot(
        &config.worker.mooglemail.diagnostics.directory,
        super::days_to_duration(ordinary_days)?,
        super::days_to_duration(hold_days)?,
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
