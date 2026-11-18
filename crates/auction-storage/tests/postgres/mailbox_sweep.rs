//! MoogleMail 邮箱维护的本地证据读写经受观测池执行（运行账号）。
//!
//! 每次调用都必须计入池的 acquisition 观测并占用普通业务配额，不借用原生池（原生池会越过
//! readiness 与长期 owner 的保留连接）；空库上的各条 SQL 同时证明与迁移后的 schema 一致。
#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::mail::{MoogleMailboxEvidenceStore, MoogleMailboxSweepPosition};
use auction_time::Timestamp;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::support;

#[tokio::test]
async fn postgres_mailbox_sweep_queries_run_through_tracked_pool() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let metrics = store.pool_metrics_source();
    let acquires = || metrics.snapshot().acquires;

    let before = acquires();
    let batch = store.unproven_sends("main", 10).await.expect("读取未证明发送");
    assert!(batch.candidates.is_empty(), "空库没有待维护的投递");
    assert!(batch.position.is_none());
    assert!(acquires() > before, "未证明发送查询必须经受观测池取连接");

    let position = MoogleMailboxSweepPosition {
        delivery_id: Uuid::new_v4(),
        created_at: Timestamp::UNIX_EPOCH,
    };
    sqlx::query(
        "INSERT INTO public.mailbox_sweep_cursors (world_id, last_created_at, last_delivery_id)
         VALUES ('main', $1, $2)",
    )
    .bind(position.created_at)
    .bind(position.delivery_id)
    .execute(store.postgres_pool())
    .await
    .expect("保留已处理投递的轮转位置");
    let batch = store.unproven_sends("main", 10).await.expect("同时读游标与空候选");
    assert!(batch.candidates.is_empty());
    assert_eq!(batch.position, Some(position));

    let before = acquires();
    let advanced = store.record_sweep_attempt("main", Uuid::new_v4()).await.expect("推进维护轮转");
    assert!(!advanced, "不存在的投递不能推进轮转游标");
    assert!(acquires() > before, "轮转游标写入必须经受观测池取连接");

    let before = acquires();
    let written = store
        .record_sent_observation(Uuid::new_v4(), 1, json!({ "matched": false }))
        .await
        .expect("写入 Sent 观察");
    assert!(!written, "投递不存在时影响零行");
    assert!(acquires() > before, "Sent 观察写入必须经受观测池取连接");

    let before = acquires();
    let written = store
        .record_sent_sighting(Uuid::new_v4(), 1, "123", json!({ "matched": true }))
        .await
        .expect("写入 Sent 目击");
    assert!(!written, "投递不存在时影响零行");
    assert!(acquires() > before, "Sent 目击写入必须经受观测池取连接");

    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_sent_sweep_preserves_observed_equipment_after_collection() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let delivery_id = Uuid::new_v4();
    let obligation_id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, failure_code, completed_at)
         VALUES ($1, $2, decode(repeat('44', 32), 'hex'), 'settlement', $3, 'FixtureBuyer',
                 $4, 'manual_reconcile', 2, 'write_unknown', clock_timestamp())",
    )
    .bind(delivery_id)
    .bind(Uuid::new_v4())
    .bind(&obligation_id)
    .bind(json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "template": "settlement_equipment_handoff",
        "variables": {"reference": obligation_id, "item_name": "Fixture Sword"},
        "credits": 0,
        "cod": 0,
        "equipment_secret_refs": [{"kind": "auction_equipment_key", "id": Uuid::new_v4()}],
    }))
    .execute(pool)
    .await
    .expect("写入待核对的结算投递");

    for (kind, mid) in [("mailbox_sweep", "7000001"), ("sent_reference_sighting", "7000002")] {
        let store = &store;
        let write = |fence, mid, observation| async move {
            let written = if kind == "mailbox_sweep" {
                store.record_sent_observation(delivery_id, fence, observation).await
            } else {
                store.record_sent_sighting(delivery_id, fence, mid, observation).await
            }
            .expect("Worker 尝试写入观察");
            assert_eq!(written, fence == 2);
        };
        let read = || async {
            sqlx::query_scalar::<_, Value>(
                "SELECT result_metadata FROM public.deliveries WHERE id=$1",
            )
            .bind(delivery_id)
            .fetch_one(pool)
            .await
            .expect("读取投递观察")[kind]
                .clone()
        };
        let ids = json!(["100", "20"]);
        write(
            2,
            mid,
            json!({
                "mid": mid,
                "collected": false,
                "remaining_equipment_count": 2,
                "remaining_equipment_ids": ids,
            }),
        )
        .await;
        let attached = read().await;
        assert_eq!(attached["remaining_equipment_ids"], ids);
        assert!(attached.get("pre_collection_equipment_ids").is_none());

        let collected = |mid| {
            json!({
                "mid": mid,
                "collected": true,
                "remaining_equipment_count": 0,
                "remaining_equipment_ids": [],
            })
        };
        write(1, mid, collected(mid)).await;
        assert_eq!(read().await, attached, "旧 fence 的观察不能覆盖当前记录");
        for _ in 0..2 {
            write(2, mid, collected(mid)).await;
            let observed = read().await;
            assert_eq!(observed["remaining_equipment_ids"], json!([]));
            assert_eq!(observed["pre_collection_equipment_ids"], ids);
        }
        let other_mid = "7000099";
        write(2, other_mid, collected(other_mid)).await;
        assert!(
            read().await.get("pre_collection_equipment_ids").is_none(),
            "不同 MID 不共享旧附件"
        );
    }
    let mid = "7000003";
    store
        .record_sent_observation(
            delivery_id,
            2,
            json!({"mid": mid, "collected": false, "remaining_equipment_ids": ["100"]}),
        )
        .await
        .expect("领取前由维护扫描观察");
    assert!(
        store
            .record_sent_sighting(
                delivery_id,
                2,
                mid,
                json!({"mid": mid, "collected": true, "remaining_equipment_ids": []}),
            )
            .await
            .expect("同一 MID 领取后由目击观察")
    );
    let metadata: Value =
        sqlx::query_scalar("SELECT result_metadata FROM public.deliveries WHERE id=$1")
            .bind(delivery_id)
            .fetch_one(pool)
            .await
            .expect("读取目击");
    assert_eq!(metadata["sent_reference_sighting"]["pre_collection_equipment_ids"], json!(["100"]));
    pool.close().await;
}
