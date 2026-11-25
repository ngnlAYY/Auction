//! 寄售派发领取：只领寄售义务的待起写轮，返回轮键与冻结身份；并发两个领取者只有一个拿到；
//! 与通用拍卖领取互不越界（拍卖义务零变化）。

use auction_application::consignment::{DispatchWorkClaim, DispatchWorkStore};
use auction_application::settlement::SettlementFulfillmentStore;
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::{OBLIGATION_1, OBLIGATION_2, OBLIGATION_3};
use super::super::deposit::support::succeeds;
use super::fixture::{CONTENT, PRICE, Seeded, frozen, round_key, uuid, work_store};

async fn claim(seeded: &Seeded) -> Option<DispatchWorkClaim> {
    let now = Timestamp::now();
    work_store(&seeded.runtime, CONTENT)
        .claim_dispatch_work(now, now + Duration::minutes(5))
        .await
        .expect("寄售派发领取")
}

/// 一条最小的 ready 拍卖义务（与通用领取的既有夹具同形）。
async fn seed_auction_obligation(pool: &PgPool) -> Uuid {
    let obligation = Uuid::new_v4();
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.auctions
                (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
                 scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
             SELECT 'd0000000-0000-4000-8000-000000000001', 'main', 1, 'Fake Auction', 'settled',
                    '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                    '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                    '{policy}'::jsonb;
             INSERT INTO public.equipment (id, world_id, external_equipment_id, name, category,
                                           collected_at)
             VALUES ('d0000000-0000-4000-8000-000000000002', 'main', '90042', 'Fake Equipment',
                     'Weapon', '2026-01-01T00:00:00Z');
             INSERT INTO public.auction_commands (command_id, command_kind, aggregate_id,
                                                  request_hash, result_code)
             VALUES ('d0000000-0000-4000-8000-000000000003', 'fake_settlement',
                     'd0000000-0000-4000-8000-000000000001', decode(repeat('11', 32), 'hex'),
                     'accepted');
             INSERT INTO public.lots (id, auction_id, equipment_id, lot_number, status, start_price,
                                      category_key, category_number, created_at, updated_at)
             VALUES ('d0000000-0000-4000-8000-000000000004',
                     'd0000000-0000-4000-8000-000000000001',
                     'd0000000-0000-4000-8000-000000000002', 1, 'unsold', 50000, 'Weapon', 1,
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO public.settlement_obligations
                (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                 obligation_kind, business_state, recipient_game_identity, amount, equipment_id)
             VALUES ('{obligation}', 'd0000000-0000-4000-8000-000000000001',
                     'd0000000-0000-4000-8000-000000000004', 'main',
                     'd0000000-0000-4000-8000-000000000003', 1, 'unsold_return', 'ready',
                     'FakeSeller', 0, 'd0000000-0000-4000-8000-000000000002');",
            policy = crate::support::fixtures::policy_snapshot(
                Timestamp::parse_from_rfc3339("2026-01-01T00:00:00Z").expect("时间"),
                50_000,
                30_000
            )
        ),
    )
    .await;
    obligation
}

#[tokio::test]
async fn postgres_consignment_dispatch_claim_gives_one_round_to_one_claimer_and_skips_auctions() {
    let seeded = Seeded::purchased().await;
    let pool = seeded.pool();
    let auction = seed_auction_obligation(pool).await;

    // 两个领取者并发：恰好一个拿到首轮，带轮键与轮上冻结的身份。
    let (first, second) = tokio::join!(claim(&seeded), claim(&seeded));
    let claims: Vec<_> = [first, second].into_iter().flatten().collect();
    assert_eq!(claims.len(), 1, "{claims:?}");
    let work = &claims[0];
    assert_eq!((work.obligation_id, work.key), (uuid(OBLIGATION_1), round_key()));
    assert_eq!(work.frozen, frozen(PRICE));
    // 反例：租约期内不再领到同一项；卖款、退款没有轮，也不被领取。
    assert!(claim(&seeded).await.is_none());
    let untouched: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_obligation_fulfillments
         WHERE obligation_id = ANY($1) AND claim_token IS NULL",
    )
    .bind(vec![uuid(OBLIGATION_2), uuid(OBLIGATION_3)])
    .fetch_one(pool)
    .await
    .expect("卖款、退款履约行");
    assert_eq!(untouched, 2);

    // 寄售领取不建、不改拍卖义务的履约行。
    let auction_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(auction)
    .fetch_one(pool)
    .await
    .expect("拍卖履约行");
    assert_eq!(auction_rows, 0);

    // 通用领取不领寄售义务，寄售义务上的 claim 原样保留。
    let now = Timestamp::now();
    let generic = SettlementFulfillmentStore::claim_due(
        &seeded.runtime,
        "generic-worker".to_owned(),
        now,
        now + Duration::minutes(5),
        10,
    )
    .await
    .expect("通用领取");
    assert!(generic.iter().all(|claim| claim.obligation_id.as_uuid() != work.obligation_id));
    // 通用领取照旧领取拍卖义务（写入 claim fence）；这条最小夹具缺拍卖上下文，领取后按既有规则
    // 隔离为 invalid_fulfillment_fact，所以只断言它确被通用路径领取，而不要求出现在返回列表里。
    let auction_fence: i64 = sqlx::query_scalar(
        "SELECT claim_fencing FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(auction)
    .fetch_one(pool)
    .await
    .expect("通用领取建立的拍卖履约行");
    assert_eq!(auction_fence, 1, "通用领取照旧领取拍卖义务");
    let token: Option<Uuid> = sqlx::query_scalar(
        "SELECT claim_token FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(work.obligation_id)
    .fetch_one(pool)
    .await
    .expect("寄售履约行");
    assert_eq!(token, Some(work.claim_token));
}
