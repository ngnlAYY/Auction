//! PostgreSQL 18 集成测试骨架（W6.2）。
//!
//! 只有 `test-db` feature 才编译并运行本文件；`just verify` 的 all-targets 检查在这里
//! 只会看到一个空 crate，不连接数据库。入口见 `just test-db`。
//!
//! `support` 负责临时集群、模板库迁移与逐测试克隆库；测试本身只使用公开的存储入口。
#![cfg(feature = "test-db")]

mod support;

use auction_application::WorldReader;
use auction_storage::postgres_expected_migration_version;

/// 空库执行全部内嵌迁移后，通过世界目录端口读取迁移种子数据。
#[tokio::test]
async fn postgres_migrates_empty_database_and_reads_worlds() {
    let database = support::TestDatabase::create_empty().await.expect("创建空测试数据库");
    let store = database.store().await.expect("连接空测试数据库");
    store.migrate().await.expect("空数据库执行全部迁移");

    let applied: i64 =
        sqlx::query_scalar("SELECT coalesce(max(version), 0) FROM public._sqlx_migrations")
            .fetch_one(store.postgres_pool())
            .await
            .expect("读取已应用迁移版本");
    assert_eq!(
        applied,
        postgres_expected_migration_version(),
        "空库迁移后应达到二进制内嵌最高版本"
    );

    let worlds = store.list_worlds().await.expect("通过世界目录端口读取世界");
    let main = worlds.iter().find(|world| world.id == "main").expect("迁移应写入 main 世界");
    assert!(main.enabled, "main 世界默认启用");
    assert_eq!(main.availability, "available");
    assert!(main.policy_revision > 0);
    assert!(worlds.iter().any(|world| world.id == "isekai"), "迁移应写入 isekai 世界");

    // 重复执行迁移必须是幂等的，不能再次改写已应用的历史。
    store.migrate().await.expect("重复执行迁移保持幂等");
    store.postgres_pool().close().await;
}

/// 每个测试从已迁移模板库克隆独立数据库，且克隆库不携带其他测试的数据。
#[tokio::test]
async fn postgres_clones_migrated_template_per_test() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");

    let applied: i64 =
        sqlx::query_scalar("SELECT coalesce(max(version), 0) FROM public._sqlx_migrations")
            .fetch_one(store.postgres_pool())
            .await
            .expect("读取克隆库迁移版本");
    assert_eq!(applied, postgres_expected_migration_version());

    let auctions: i64 = sqlx::query_scalar("SELECT count(*) FROM public.auctions")
        .fetch_one(store.postgres_pool())
        .await
        .expect("读取克隆库场次行数");
    assert_eq!(auctions, 0, "克隆库只包含模板库的 schema 与迁移种子");

    store.postgres_pool().close().await;
}
