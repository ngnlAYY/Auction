//! 通用投递路径不处理寄售轮投递：领取与起写都被排除。
//!
//! 保护不变量 4：寄售轮的 Send 只经派发端口的 `mark_write_started` 持久起写；通用领取或通用 sending
//! 围栏若接管同一投递，就会在不登记寄售尝试的情况下产生第二条 POST 路径。

use auction_application::core::RequestHash;
use auction_application::delivery::{
    DeliveryClaimExclusions, DeliveryFenceOutcome, DeliveryPreparedExpectation,
    DeliverySendExpectation, DeliveryWorkClaim, DeliveryWorkStore,
};
use auction_time::{Duration, Timestamp};
use serde_json::Value;
use uuid::Uuid;

use super::fixture::{PRICE, Seeded};

/// 购买事务建立的首轮投递一直是 `prepared`；通用路径只看 `pending`，这里与激活后的状态一致。
async fn activate(seeded: &Seeded) {
    sqlx::query(
        "UPDATE public.deliveries
         SET status = 'pending', activation_command_id = gen_random_uuid(),
             activation_request_digest = decode(repeat('ab', 32), 'hex'),
             activated_at = statement_timestamp()
         WHERE id = $1 AND status = 'prepared'",
    )
    .bind(seeded.delivery_id)
    .execute(seeded.pool())
    .await
    .expect("激活投递");
}

async fn status(seeded: &Seeded) -> String {
    sqlx::query_scalar("SELECT status FROM public.deliveries WHERE id = $1")
        .bind(seeded.delivery_id)
        .fetch_one(seeded.pool())
        .await
        .expect("投递状态")
}

#[tokio::test]
async fn postgres_consignment_dispatch_generic_claim_skips_a_pending_round_delivery() {
    let seeded = Seeded::purchased().await;
    activate(&seeded).await;
    let now = Timestamp::now();
    let grace = std::time::Duration::from_secs(60);
    let store = &seeded.runtime;
    store.begin_claim_cycle(now, grace).await.expect("领取前清理");
    let claims = store
        .claim_due(
            "consignment-round-guard".to_owned(),
            now,
            now + Duration::minutes(2),
            grace,
            DeliveryClaimExclusions::default(),
            10,
        )
        .await
        .expect("领取投递");
    // 守卫判据的等价性：每个轮投递的载荷都带冻结身份摘要（见 `delivery_not_consignment_round_guard`）。
    let without_digest: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_dispatch_rounds AS round
         JOIN public.deliveries AS delivery ON delivery.id = round.delivery_id
         WHERE delivery.payload ->> 'identity_digest' IS NULL",
    )
    .fetch_one(seeded.pool())
    .await
    .expect("读取轮投递载荷");
    assert_eq!(without_digest, 0);
    assert!(
        claims.iter().all(|claim| claim.delivery_id != seeded.delivery_id),
        "寄售轮投递不被通用领取：{claims:?}"
    );
    assert_eq!(status(&seeded).await, "pending");
}

#[tokio::test]
async fn postgres_consignment_dispatch_generic_fence_cannot_start_a_round_delivery() {
    let seeded = Seeded::purchased().await;
    seeded.claim(Uuid::new_v4(), 1).await;
    // 以投递行的真实事实组装 claim：通用围栏对它的拒绝只能来自来源排除，而非 claim 不符。
    #[allow(clippy::type_complexity, reason = "只在本测试里一次性读取投递行")]
    let row: (
        Uuid,
        Vec<u8>,
        String,
        String,
        String,
        Value,
        Uuid,
        Timestamp,
        i64,
        i32,
        Timestamp,
    ) = sqlx::query_as(
        "SELECT command_id, request_digest, origin_kind, origin_id, recipient, payload,
                    lease_token, lease_expires_at, fencing_version, attempt_count, created_at
             FROM public.deliveries WHERE id = $1",
    )
    .bind(seeded.delivery_id)
    .fetch_one(seeded.pool())
    .await
    .expect("读取投递行");
    let digest: [u8; 32] = row.1.try_into().expect("请求摘要 32 字节");
    let claim = DeliveryWorkClaim {
        delivery_id: seeded.delivery_id,
        command_id: row.0,
        request_hash: RequestHash::new(digest),
        origin_kind: row.2,
        origin_id: row.3,
        recipient: row.4,
        payload: row.5,
        lease_token: row.6,
        lease_until: row.7,
        fencing_version: row.8,
        attempt_count: u32::try_from(row.9).expect("计数"),
        worker_id: "consignment-round-guard".to_owned(),
        claimed_at: Timestamp::now(),
        created_at: row.10,
    };
    // 与 claim 一致的冻结 Send 身份：围栏若被拒，原因只能是来源排除，而不是身份不符。
    let expectation =
        DeliveryPreparedExpectation::Send(std::sync::Arc::new(DeliverySendExpectation {
            world_id: "main".to_owned(),
            recipient: claim.recipient.clone(),
            subject: "fake subject".to_owned(),
            body_digest: [0xd1; 32],
            credits: 0,
            cod: PRICE,
            equipment_ids: vec!["42".to_owned()],
            credential: None,
        }));
    let outcome = seeded
        .runtime
        .start_sending_authorized(claim, Timestamp::now(), None, Some(&expectation))
        .await;
    assert!(!matches!(outcome, Ok(DeliveryFenceOutcome::Sending)), "{outcome:?}");
    assert_eq!(status(&seeded).await, "claimed");
    let attempts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.delivery_send_attempts WHERE delivery_id = $1",
    )
    .bind(seeded.delivery_id)
    .fetch_one(seeded.pool())
    .await
    .expect("尝试数");
    assert_eq!(attempts, 0);
}
