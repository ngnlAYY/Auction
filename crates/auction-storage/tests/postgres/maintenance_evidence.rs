//! 运维只读证据（保留盘点、恢复预检）共用迁移 owner 的 schema 身份与迁移历史读取。
//!
//! 已迁移的模板库上两条入口都报告有效 schema 身份与完整的当前迁移历史；schema 身份损坏时
//! 两者都按身份无效失败关闭；从未迁移的空库没有迁移历史标记，盘点拒绝、预检如实报告缺失。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。
#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_storage::{DatabaseStore, RecoveryPreflightError, RetentionInventoryError};

use crate::support;

async fn database_name(store: &DatabaseStore) -> String {
    sqlx::query_scalar("SELECT current_database()")
        .fetch_one(store.postgres_pool())
        .await
        .expect("读取数据库名")
}

#[tokio::test]
async fn postgres_maintenance_evidence_reads_schema_identity_and_current_history() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.maintenance_store().await.expect("以维护账号连接");
    let name = database_name(&store).await;

    let inventory = store.collect_retention_inventory(&name).await.expect("保留盘点");
    assert_eq!(inventory.schema_identity.application, "auction");
    assert!(inventory.schema_identity.baseline_version > 0);
    assert!(inventory.migration.current, "模板库已执行全部内嵌迁移");
    assert!(inventory.migration.all_success);
    assert_eq!(inventory.migration.latest_version, inventory.migration.expected_version);

    let preflight = store.collect_recovery_preflight(&name).await.expect("恢复预检");
    assert!(preflight.schema_identity_present && preflight.migration_history_present);
    let identity = preflight.schema_identity.expect("已迁移库有 schema 身份");
    assert_eq!(
        (identity.application.as_str(), identity.baseline_version),
        ("auction", inventory.schema_identity.baseline_version)
    );
    let migration = preflight.migration.expect("已迁移库有迁移历史");
    assert!(migration.current);
    assert_eq!(migration.migration_count, inventory.migration.migration_count);
    assert!(!preflight.restore_preflight_passed, "已有 schema 的库不能作为恢复目标");

    sqlx::query("UPDATE public.auction_schema_identity SET application = 'other'")
        .execute(store.postgres_pool())
        .await
        .expect("改写 schema 身份");
    assert!(matches!(
        store.collect_retention_inventory(&name).await,
        Err(RetentionInventoryError::SchemaIdentityInvalid)
    ));
    assert!(matches!(
        store.collect_recovery_preflight(&name).await,
        Err(RecoveryPreflightError::SchemaIdentityInvalid)
    ));

    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_maintenance_evidence_reports_missing_migration_history_on_empty_database() {
    let database = support::TestDatabase::create_empty().await.expect("新建空库");
    let store = database.maintenance_store().await.expect("以维护账号连接");
    let name = database_name(&store).await;

    assert!(matches!(
        store.collect_retention_inventory(&name).await,
        Err(RetentionInventoryError::MigrationHistoryMissing)
    ));
    let preflight = store.collect_recovery_preflight(&name).await.expect("空库恢复预检");
    assert!(!preflight.schema_identity_present && preflight.schema_identity.is_none());
    assert!(!preflight.migration_history_present && preflight.migration.is_none());

    store.postgres_pool().close().await;
}
