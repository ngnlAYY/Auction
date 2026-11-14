//! MoogleMail 邮箱维护的本地证据读写经受观测池执行（运行账号）。
//!
//! 每次调用都必须计入池的 acquisition 观测并占用普通业务配额，不借用原生池（原生池会越过
//! readiness 与长期 owner 的保留连接）；空库上的各条 SQL 同时证明与迁移后的 schema 一致。
#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::mail::MoogleMailboxEvidenceStore;
use serde_json::json;
use uuid::Uuid;

use crate::support;

#[tokio::test]
async fn postgres_mailbox_sweep_queries_run_through_tracked_pool() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let metrics = store.pool_metrics_source();
    let acquires = || metrics.snapshot().acquires;

    let before = acquires();
    let candidates = store.unproven_sends("main", 10).await.expect("读取未证明发送");
    assert!(candidates.is_empty(), "空库没有待维护的投递");
    assert!(acquires() > before, "未证明发送查询必须经受观测池取连接");

    let before = acquires();
    let advanced = store.record_sweep_attempt("main", Uuid::new_v4()).await.expect("推进维护轮转");
    assert!(!advanced, "不存在的投递不能推进轮转游标");
    assert!(acquires() > before, "轮转游标写入必须经受观测池取连接");

    let before = acquires();
    store
        .record_sent_observation(Uuid::new_v4(), 1, json!({ "matched": false }))
        .await
        .expect("写入 Sent 观察");
    assert!(acquires() > before, "Sent 观察写入必须经受观测池取连接");

    let before = acquires();
    store
        .record_sent_sighting(Uuid::new_v4(), 1, "123", json!({ "matched": true }))
        .await
        .expect("写入 Sent 目击");
    assert!(acquires() > before, "Sent 目击写入必须经受观测池取连接");

    store.postgres_pool().close().await;
}
