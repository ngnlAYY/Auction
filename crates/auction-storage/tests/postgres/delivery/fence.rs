//! 发送围栏之后的收口：成功证明绑定唯一 Sent MID、同一成功事实重放、MID 冲突、远端结果未知
//! 转人工、未过围栏的领取不得写回外部结果，以及过期租约不得续租或释放。
//!
//! 只经 `DeliveryWorkStore` 端口按 Worker 的真实顺序驱动：领取 → `start_sending_authorized`
//! （无现金证据、带冻结 Send 身份）→ `succeed`/`fail`，或领取后的 `renew`/`release`。保护的
//! 不变量：外部结果只在本地 sending 提交后写回；一个 world/MID 只证明一个 delivery/fence；
//! 同一成功事实重放不追加审计；结果未知只转人工核对；过期租约不复活。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use auction_application::delivery::{
    DeliveryClaimExclusions, DeliveryFenceOutcome, DeliveryPreparedExpectation,
    DeliverySendExpectation, DeliverySendResult, DeliveryTransportError, DeliveryWorkClaim,
    DeliveryWorkStore,
};
use auction_storage::DatabaseStore;
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::support;

const WORKER: &str = "delivery-fence-worker";

/// 插入一条 Main 世界、不带 Credits/COD/装备的手工 Send；`manual` 来源不经过通知资格门。
async fn insert_send(pool: &PgPool, recipient: &str) -> Uuid {
    let delivery_id = Uuid::new_v4();
    let payload = json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "external_mail_id": delivery_id.to_string(),
        "template": "manual_notice",
        "result_code": "test",
        "user_key_user_id": null,
        "credits": 0,
        "cod": 0,
        "attachment_ids": [],
        "body": "Frozen body",
    });
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status)
         VALUES ($1, $2, decode(repeat('33', 32), 'hex'), 'manual', $3, $4, $5, 'pending')",
    )
    .bind(delivery_id)
    .bind(Uuid::new_v4())
    .bind(format!("fence:{delivery_id}"))
    .bind(recipient)
    .bind(&payload)
    .execute(pool)
    .await
    .expect("写入待发送投递");
    delivery_id
}

/// 领取全部到期投递，租约两分钟。
async fn claim_due(store: &DatabaseStore) -> Vec<DeliveryWorkClaim> {
    let now = Utc::now();
    let sending_grace = std::time::Duration::from_secs(60);
    store.begin_claim_cycle(now, sending_grace).await.expect("执行领取前清理");
    store
        .claim_due(
            WORKER.to_owned(),
            now,
            now + Duration::minutes(2),
            sending_grace,
            DeliveryClaimExclusions::default(),
            10,
        )
        .await
        .expect("领取投递")
}

fn claim_for(claims: &[DeliveryWorkClaim], delivery_id: Uuid) -> DeliveryWorkClaim {
    claims
        .iter()
        .find(|claim| claim.delivery_id == delivery_id)
        .cloned()
        .expect("目标投递必须被领取")
}

/// 与传输边界同形的冻结 Send 身份。
fn send_expectation(claim: &DeliveryWorkClaim) -> Arc<DeliverySendExpectation> {
    Arc::new(DeliverySendExpectation {
        world_id: "main".to_owned(),
        recipient: claim.recipient.clone(),
        subject: "Fence notice".to_owned(),
        body_digest: [9_u8; 32],
        credits: 0,
        cod: 0,
        equipment_ids: Vec::new(),
        credential: None,
    })
}

/// 以冻结 Send 身份进入 sending，返回该身份供成功证明引用。
async fn fence(store: &DatabaseStore, claim: &DeliveryWorkClaim) -> Arc<DeliverySendExpectation> {
    let expectation = send_expectation(claim);
    let prepared = DeliveryPreparedExpectation::Send(Arc::clone(&expectation));
    let outcome = store
        .start_sending_authorized(claim.clone(), Utc::now(), None, Some(&prepared))
        .await
        .expect("发送围栏必须确定完成");
    assert!(matches!(outcome, DeliveryFenceOutcome::Sending), "合法冻结身份必须进入 sending");
    expectation
}

/// 传输边界在 Sent 列表核验同一冻结请求后产生的成功证明。
fn sent_result(
    claim: &DeliveryWorkClaim,
    expectation: &DeliverySendExpectation,
    external_mail_id: &str,
) -> DeliverySendResult {
    DeliverySendResult {
        external_reference: format!("mooglemail:main:send:{external_mail_id}"),
        result_metadata: json!({
            "operation": "send",
            "external_reference_proven": true,
            "sent_verified": true,
            "proof": "sent_exact_payload_v1",
            "sending_fencing_version": claim.fencing_version,
            "send_expectation": serde_json::to_value(expectation).expect("序列化冻结身份"),
        }),
    }
}

async fn audits(pool: &PgPool, delivery_id: Uuid, action: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.audit_records
         WHERE entity_type = 'delivery' AND entity_id = $1 AND action = $2",
    )
    .bind(delivery_id.to_string())
    .bind(action)
    .fetch_one(pool)
    .await
    .expect("读取投递审计")
}

/// 状态、外部引用、租约令牌与完成时间。
async fn delivery_row(
    pool: &PgPool,
    delivery_id: Uuid,
) -> (String, Option<String>, Option<Uuid>, Option<DateTime<Utc>>) {
    sqlx::query_as(
        "SELECT status, external_reference, lease_token, completed_at
         FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery_id)
    .fetch_one(pool)
    .await
    .expect("读取投递状态")
}

async fn sent_bindings(pool: &PgPool, external_mail_id: &str) -> Vec<(Uuid, i64, Option<i64>)> {
    sqlx::query_as(
        "SELECT delivery_id, fencing_version, expectation_fencing_version
         FROM public.delivery_sent_bindings
         WHERE world_id = 'main' AND external_mail_id = $1",
    )
    .bind(external_mail_id)
    .fetch_all(pool)
    .await
    .expect("读取 Sent 绑定")
}

#[tokio::test]
async fn postgres_sent_proof_binds_one_mid_and_identical_replay_adds_no_audit() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let delivery_id = insert_send(pool, "Fence Player").await;
    let claim = claim_for(&claim_due(&store).await, delivery_id);
    let expectation = fence(&store, &claim).await;

    let result = sent_result(&claim, &expectation, "1001");
    store.succeed(claim.clone(), result.clone(), Utc::now()).await.expect("合法 Sent 证明必须收口");

    let (status, reference, lease_token, completed_at) = delivery_row(pool, delivery_id).await;
    assert_eq!(status, "succeeded");
    assert_eq!(reference.as_deref(), Some("mooglemail:main:send:1001"));
    assert!(lease_token.is_none(), "终态必须释放租约");
    assert_eq!(
        sent_bindings(pool, "1001").await,
        vec![(delivery_id, claim.fencing_version, Some(claim.fencing_version))],
        "MID 必须唯一绑定到本次冻结尝试"
    );
    assert_eq!(audits(pool, delivery_id, "delivery.succeed").await, 1);

    // 提交应答丢失后的同一事实重放：返回成功，但不追加审计、不改写终态与绑定。
    store.succeed(claim.clone(), result, Utc::now()).await.expect("同一成功事实重放必须幂等");
    assert_eq!(audits(pool, delivery_id, "delivery.succeed").await, 1, "重放不得追加成功审计");
    assert_eq!(
        delivery_row(pool, delivery_id).await,
        ("succeeded".to_owned(), reference, None, completed_at),
        "重放不得改写已提交的终态"
    );
    assert_eq!(sent_bindings(pool, "1001").await.len(), 1);

    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_sent_proof_rejects_a_mid_already_bound_to_another_delivery() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let first_id = insert_send(pool, "Fence First").await;
    let second_id = insert_send(pool, "Fence Second").await;
    let claims = claim_due(&store).await;
    let first = claim_for(&claims, first_id);
    let second = claim_for(&claims, second_id);
    let first_expectation = fence(&store, &first).await;
    let second_expectation = fence(&store, &second).await;

    store
        .succeed(first.clone(), sent_result(&first, &first_expectation, "2001"), Utc::now())
        .await
        .expect("首个投递证明 MID");
    let rejected = store
        .succeed(second.clone(), sent_result(&second, &second_expectation, "2001"), Utc::now())
        .await;
    assert!(rejected.is_err(), "同一 world/MID 不能再证明另一投递");

    let (status, reference, lease_token, completed_at) = delivery_row(pool, second_id).await;
    assert_eq!(status, "sending", "拒绝的成功证明不得改写在途发送");
    assert!(reference.is_none());
    assert_eq!(lease_token, Some(second.lease_token));
    assert!(completed_at.is_none());
    assert_eq!(
        sent_bindings(pool, "2001").await,
        vec![(first_id, first.fencing_version, Some(first.fencing_version))],
        "MID 绑定仍只指向首个投递"
    );
    assert_eq!(audits(pool, second_id, "delivery.succeed").await, 0);

    // 正对照：同一份证明换成未被占用的 MID 即可收口，说明上面的拒绝来自 MID 冲突。
    store
        .succeed(second.clone(), sent_result(&second, &second_expectation, "2002"), Utc::now())
        .await
        .expect("换成未占用的 MID 必须收口");
    assert_eq!(
        sent_bindings(pool, "2002").await,
        vec![(second_id, second.fencing_version, Some(second.fencing_version))]
    );

    store.postgres_pool().close().await;
}

/// 未过发送围栏（仍是 claimed）的领取不得写回外部结果：成功证明与确定失败都被拒绝，行保持
/// claimed，不产生 Sent 绑定与审计。
#[tokio::test]
async fn postgres_claim_without_sending_fence_cannot_record_external_outcomes() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let delivery_id = insert_send(pool, "Fence Unfenced").await;
    let claim = claim_for(&claim_due(&store).await, delivery_id);
    let expectation = send_expectation(&claim);

    assert!(
        store
            .succeed(claim.clone(), sent_result(&claim, &expectation, "3001"), Utc::now())
            .await
            .is_err(),
        "未进入 sending 的领取不得写回成功证明"
    );
    let failure = DeliveryTransportError {
        failure_code: "transport_rejected".to_owned(),
        safe_detail: "远端明确拒绝".to_owned(),
        outcome_unknown: false,
        external_reference: None,
        result_metadata: json!({"category": "transport"}),
    };
    assert!(
        store.fail(claim.clone(), failure, Utc::now()).await.is_err(),
        "未进入 sending 的领取不得写回外部失败"
    );

    let (status, reference, lease_token, completed_at) = delivery_row(pool, delivery_id).await;
    assert_eq!(status, "claimed");
    assert!(reference.is_none());
    assert_eq!(lease_token, Some(claim.lease_token));
    assert!(completed_at.is_none());
    assert!(sent_bindings(pool, "3001").await.is_empty());
    assert_eq!(audits(pool, delivery_id, "delivery.succeed").await, 0);
    assert_eq!(audits(pool, delivery_id, "delivery.fail").await, 0);

    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_unknown_send_outcome_moves_to_manual_reconcile() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let delivery_id = insert_send(pool, "Fence Unknown").await;
    let claim = claim_for(&claim_due(&store).await, delivery_id);
    fence(&store, &claim).await;

    store
        .fail(
            claim,
            DeliveryTransportError {
                failure_code: "transport_timeout".to_owned(),
                safe_detail: "发送请求超时，远端结果未知".to_owned(),
                outcome_unknown: true,
                external_reference: None,
                result_metadata: json!({"category": "transport"}),
            },
            Utc::now(),
        )
        .await
        .expect("未知结果必须收口为人工核对");

    let (status, failure, metadata): (String, Option<String>, serde_json::Value) = sqlx::query_as(
        "SELECT status, failure_code, result_metadata FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery_id)
    .fetch_one(pool)
    .await
    .expect("读取投递终态");
    assert_eq!(status, "manual_reconcile", "远端结果未知不得自动重发或判定失败");
    assert_eq!(failure.as_deref(), Some("transport_timeout"));
    assert_eq!(metadata["outcome_unknown"], json!(true));
    let (_, reference, lease_token, completed_at) = delivery_row(pool, delivery_id).await;
    assert!(reference.is_none());
    assert!(lease_token.is_none());
    assert!(completed_at.is_some());
    assert_eq!(audits(pool, delivery_id, "delivery.outcome_unknown").await, 1);
    assert_eq!(audits(pool, delivery_id, "delivery.fail").await, 0);

    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_expired_lease_cannot_be_renewed_or_released() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let delivery_id = insert_send(pool, "Fence Lease").await;
    let claim = claim_for(&claim_due(&store).await, delivery_id);

    // 对照：有效租约可以续租，证明同一领取结果本身合法。
    let renewed_at = Utc::now();
    store
        .renew(claim.clone(), renewed_at, renewed_at + Duration::minutes(3))
        .await
        .expect("有效租约可以续租");

    sqlx::query(
        "UPDATE public.deliveries SET lease_expires_at = clock_timestamp() - interval '1 second'
         WHERE id = $1",
    )
    .bind(delivery_id)
    .execute(pool)
    .await
    .expect("把租约置为已过期");
    let (expired_at, attempt_count): (DateTime<Utc>, i32) = sqlx::query_as(
        "SELECT lease_expires_at, attempt_count FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery_id)
    .fetch_one(pool)
    .await
    .expect("读取过期租约");

    let now = Utc::now();
    assert!(
        store.renew(claim.clone(), now, now + Duration::minutes(2)).await.is_err(),
        "过期租约不得续租复活"
    );
    assert!(store.release(claim.clone(), now).await.is_err(), "过期租约不得释放回 pending");

    let (status, _, lease_token, _) = delivery_row(pool, delivery_id).await;
    assert_eq!(status, "claimed");
    assert_eq!(lease_token, Some(claim.lease_token));
    let (lease_after, attempts_after): (DateTime<Utc>, i32) = sqlx::query_as(
        "SELECT lease_expires_at, attempt_count FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery_id)
    .fetch_one(pool)
    .await
    .expect("读取拒绝后的租约");
    assert_eq!(lease_after, expired_at, "拒绝的续租不得延长租约");
    assert_eq!(attempts_after, attempt_count, "拒绝的释放不得撤销尝试计数");
    assert_eq!(audits(pool, delivery_id, "delivery.release").await, 0);

    store.postgres_pool().close().await;
}
