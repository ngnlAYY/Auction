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
    let store = connect(config_dir, database).await?;
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
    let store = connect(config_dir, database).await?;
    let DatabaseStore::Postgres(postgres) = &store;
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

async fn connect(config_dir: &ConfigDir, database: &str) -> anyhow::Result<DatabaseStore> {
    let config = config_dir.load_worker()?;
    let url = config.database_url().context("Worker 配置缺少数据库连接")?;
    DatabaseTarget::parse(url)?.require_database(database)?;
    super::connect(url).await
}
