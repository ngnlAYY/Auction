//! 通用过期 `sending` 回收不碰寄售轮投递：寄售轮起写后的结果未知由寄售派发路径承担，通用回收
//! 不得加 fence、改人工对账；拍卖侧投递照旧回收。

use auction_application::delivery::DeliveryWorkStore;
use auction_time::Timestamp;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::OBLIGATION_2;
use super::super::deposit::support::succeeds;
use super::fixture::Seeded;

const AUCTION_DELIVERY: &str = "c9200000-0000-4000-8000-000000000001";

/// 投递的 (状态, fence)。
async fn state(pool: &PgPool, delivery: Uuid) -> (String, i64) {
    sqlx::query_as("SELECT status, fencing_version FROM public.deliveries WHERE id = $1")
        .bind(delivery)
        .fetch_one(pool)
        .await
        .expect("投递行")
}

#[tokio::test]
async fn postgres_consignment_dispatch_generic_expiry_leaves_consignment_rounds_alone() {
    let (seeded, _started) = Seeded::first_dispatch_started().await;
    let pool = seeded.pool();
    // 寄售轮投递已起写（sending、fence 1）且租约早已过期；另有一封同样过期的拍卖侧 sending 投递。
    succeeds(
        pool,
        &format!(
            "UPDATE public.deliveries SET lease_expires_at = clock_timestamp() - interval '1 hour'
             WHERE id = '{consignment}';
             INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, lease_token, lease_expires_at, fencing_version, attempt_count,
                 claimed_at, sending_at, created_at, updated_at)
             VALUES ('{AUCTION_DELIVERY}', 'c9200000-0000-4000-8000-000000000002',
                     decode(repeat('c1', 32), 'hex'), 'settlement', '{OBLIGATION_2}',
                     'FakeSellerTwo', '{{}}'::jsonb, 'sending', gen_random_uuid(),
                     clock_timestamp() - interval '1 hour', 1, 1,
                     '2026-01-01T04:00:00Z', '2026-01-01T04:00:00Z',
                     '2026-01-01T04:00:00Z', '2026-01-01T04:00:00Z');",
            consignment = seeded.delivery_id,
        ),
    )
    .await;
    DeliveryWorkStore::begin_claim_cycle(
        &seeded.runtime,
        Timestamp::now(),
        std::time::Duration::ZERO,
    )
    .await
    .expect("通用领取周期");
    // 反例：寄售轮投递原样停在 sending、fence 不变。
    assert_eq!(state(pool, seeded.delivery_id).await, ("sending".to_owned(), 1));
    // 正例：拍卖侧投递照旧转人工核对并推进 fence。
    let auction = Uuid::parse_str(AUCTION_DELIVERY).expect("投递 ID");
    assert_eq!(state(pool, auction).await, ("manual_reconcile".to_owned(), 2));
}
