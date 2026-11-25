//! 首轮衔接：寄售义务领取后把履约行指向轮投递并领取投递；并发两个领取者只有一个拿到租约，
//! 同一轮只产生一次起写。

use auction_application::consignment::WriteStart;
use auction_domain::consignment::dispatch::{AttemptKey, dispatch_round_id};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::OBLIGATION_1;
use super::fixture::{PRICE, Seeded, binding, frozen, round_key};

/// 模拟寄售义务领取：在履约行写入 claim token 与 fence，返回 fence。
async fn claim_obligation(pool: &PgPool, token: Uuid) -> i64 {
    sqlx::query_scalar(
        "UPDATE public.settlement_obligation_fulfillments
         SET claim_token = $2, claimed_at = clock_timestamp(),
             claim_expires_at = clock_timestamp() + interval '5 minutes',
             claim_fencing = claim_fencing + 1, state_version = state_version + 1
         WHERE obligation_id = $1::uuid
         RETURNING claim_fencing",
    )
    .bind(OBLIGATION_1)
    .bind(token)
    .fetch_one(pool)
    .await
    .expect("领取寄售义务")
}

async fn write_starts(pool: &PgPool, delivery: Uuid) -> (i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.delivery_send_attempts WHERE delivery_id = $1),
                (SELECT count(*) FROM public.consignment_dispatch_attempts WHERE delivery_id = $1)",
    )
    .bind(delivery)
    .fetch_one(pool)
    .await
    .expect("读取起写行数")
}

#[tokio::test]
async fn postgres_consignment_dispatch_first_round_link_leases_once_and_starts_once() {
    let seeded = Seeded::purchased().await;
    let pool = seeded.pool();
    let claim_token = Uuid::new_v4();
    let fencing = claim_obligation(pool, claim_token).await;
    let round_id = dispatch_round_id(&round_key());
    let obligation = Uuid::parse_str(OBLIGATION_1).expect("义务 ID");
    let lease =
        || seeded.runtime.test_lease_dispatch_round(obligation, claim_token, fencing, round_id);

    // 两个领取者同时衔接同一轮：恰好一个拿到投递租约，另一个只得到 busy。
    let (first, second) = tokio::join!(lease(), lease());
    let outcomes = [first.expect("衔接一"), second.expect("衔接二")];
    let leased: Vec<_> = outcomes.iter().filter_map(|outcome| outcome.ok()).collect();
    assert_eq!(leased.len(), 1, "{outcomes:?}");
    assert!(outcomes.contains(&Err("busy")), "{outcomes:?}");
    let (delivery_id, token, fence) = leased[0];
    assert_eq!(delivery_id, seeded.delivery_id);
    let linked: (String, Option<Uuid>) = sqlx::query_as(
        "SELECT execution_state, delivery_id FROM public.settlement_obligation_fulfillments
         WHERE obligation_id = $1::uuid",
    )
    .bind(OBLIGATION_1)
    .fetch_one(pool)
    .await
    .expect("履约行");
    assert_eq!(linked, ("enqueued".to_owned(), Some(seeded.delivery_id)));

    // 拿到租约的领取者起写一次；之后的领取只得到 settled，不再有任何起写。
    let attempt = AttemptKey { delivery_id, fence };
    let service = seeded.service();
    service.start_attempt(round_key(), attempt, token, frozen(PRICE)).await.expect("登记尝试");
    let started = service.mark_write_started(attempt, token, Some(binding())).await.expect("起写");
    assert!(matches!(started, WriteStart::FirstCommitted { .. }), "{started:?}");
    assert_eq!(lease().await.expect("再衔接"), Err("settled"));
    assert_eq!(write_starts(pool, delivery_id).await, (1, 1));
}

#[tokio::test]
async fn postgres_consignment_dispatch_first_round_link_rejects_a_stale_obligation_claim() {
    let seeded = Seeded::purchased().await;
    let pool = seeded.pool();
    let stale = Uuid::new_v4();
    let stale_fencing = claim_obligation(pool, stale).await;
    // 被他人重新领取后，旧 claim 不能衔接、不能领取投递。
    claim_obligation(pool, Uuid::new_v4()).await;
    let obligation = Uuid::parse_str(OBLIGATION_1).expect("义务 ID");
    let outcome = seeded
        .runtime
        .test_lease_dispatch_round(
            obligation,
            stale,
            stale_fencing,
            dispatch_round_id(&round_key()),
        )
        .await
        .expect("衔接");
    assert_eq!(outcome, Err("rejected"));
    let status: String = sqlx::query_scalar("SELECT status FROM public.deliveries WHERE id = $1")
        .bind(seeded.delivery_id)
        .fetch_one(pool)
        .await
        .expect("投递状态");
    assert_eq!(status, "prepared");
}
