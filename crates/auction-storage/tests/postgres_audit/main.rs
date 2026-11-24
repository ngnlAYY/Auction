//! 独立进程运行会写入审计健康计数的 PG18 集成测试。

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod consignment;

// 复用 postgres 的夹具；空库迁移入口和其他场景的种子由 postgres 目标使用。
#[allow(dead_code)]
mod support;

use crate::support::TestDatabase;

#[tokio::test]
async fn postgres_audit_failure_rolls_back() {
    let database = TestDatabase::create().await.expect("建临时数据库失败");
    let maintenance = database.maintenance_store().await.expect("连接维护账号失败");
    let store = database.store().await.expect("连接运行账号失败");
    store
        .test_consignment_transactions(&maintenance, "audit_failure_rolls_back")
        .await
        .expect("寄售审计失败回滚场景失败");
}
