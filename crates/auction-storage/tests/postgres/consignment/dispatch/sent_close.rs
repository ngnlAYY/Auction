//! Sent 证据收口：Sent 绑定证据与轮投递 `sending → succeeded` 同事务，重放不重复推进；
//! 投递已不在 `sending` 时证据照记、不覆盖已有终态。发送读口给出冻结内容。

use auction_application::consignment::{DispatchWorkStore, RecordEvidenceInput, WriteStart};
use auction_domain::consignment::dispatch::{AttemptKey, EvidenceCategory, dispatch_round_id};
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::fixture::{CONTENT, PRICE, Seeded, at, binding, frozen, round_key, work_store};

const MID: &str = "9001";

/// 投递的 (状态, 外部引用, 完成时刻, 租约)。
type DeliveryRow = (String, Option<String>, Option<Timestamp>, Option<Uuid>);

async fn delivery_row(pool: &PgPool, delivery: Uuid) -> DeliveryRow {
    sqlx::query_as(
        "SELECT status, external_reference, completed_at, lease_token
         FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery)
    .fetch_one(pool)
    .await
    .expect("投递行")
}

/// (成功审计数, 起写数, 证据命令数)：起写数即远端 POST 许可数。
async fn tallies(pool: &PgPool, delivery: Uuid) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.audit_records
                 WHERE entity_id = $1::text AND action = 'delivery.succeed'),
                (SELECT count(*) FROM public.delivery_send_attempts WHERE delivery_id = $1),
                (SELECT count(*) FROM public.consignment_commands
                 WHERE operation = 'record_evidence')",
    )
    .bind(delivery)
    .fetch_one(pool)
    .await
    .expect("计数")
}

fn sent(attempt: AttemptKey) -> RecordEvidenceInput {
    RecordEvidenceInput {
        attempt,
        category: EvidenceCategory::SentBinding,
        remote_observation_id: MID.to_owned(),
        observed_at: at("2026-01-01T05:00:00Z"),
        validator_version: "fake-validator-v1".to_owned(),
        evidence_digest: [7; 32],
    }
}

#[tokio::test]
async fn postgres_consignment_dispatch_sent_evidence_closes_the_round_delivery_once() {
    let seeded = Seeded::purchased().await;
    let pool = seeded.pool();
    let now = Timestamp::now();
    let work = work_store(&seeded.runtime, CONTENT)
        .claim_dispatch_work(now, now + Duration::minutes(5))
        .await
        .expect("领取")
        .expect("有待派发的首轮");
    let (delivery, token, fence) = seeded
        .runtime
        .test_lease_dispatch_round(
            work.obligation_id,
            work.claim_token,
            work.claim_fencing,
            dispatch_round_id(&work.key),
        )
        .await
        .expect("衔接")
        .expect("取得投递租约");
    // 发送读口：收件人与冻结身份取轮与投递的冻结内容。
    let request = work_store(&seeded.runtime, CONTENT)
        .dispatch_send_request(delivery)
        .await
        .expect("读口")
        .expect("已衔接");
    assert_eq!((request.recipient.as_str(), request.key), ("FakeSellerTwo", round_key()));
    assert_eq!(request.frozen, frozen(PRICE));

    let attempt = AttemptKey { delivery_id: delivery, fence };
    let service = seeded.service();
    service.start_attempt(work.key, attempt, token, work.frozen.clone()).await.expect("登记尝试");
    let started = service.mark_write_started(attempt, token, Some(binding())).await.expect("起写");
    assert!(matches!(started, WriteStart::FirstCommitted { .. }), "{started:?}");
    assert_eq!(delivery_row(pool, delivery).await.0, "sending");

    // 正例：Sent 绑定证据落库的同一事务把投递推进到 succeeded。
    assert!(!service.record_evidence(sent(attempt)).await.expect("证据").replayed);
    let closed = delivery_row(pool, delivery).await;
    assert_eq!(
        (closed.0.as_str(), closed.1.as_deref(), closed.3),
        ("succeeded", Some("mooglemail:main:send:9001"), None)
    );
    // 重放：不重复推进，完成时刻与审计不变；起写恰一次、证据命令恰一条。
    assert!(service.record_evidence(sent(attempt)).await.expect("重放").replayed);
    assert_eq!(delivery_row(pool, delivery).await, closed);
    assert_eq!(tallies(pool, delivery).await, (1, 1, 1));
}

#[tokio::test]
async fn postgres_consignment_dispatch_sent_evidence_does_not_overwrite_a_reconciled_delivery() {
    let (seeded, started) = Seeded::first_dispatch_started().await;
    let pool = seeded.pool();
    // 过期回收已把 sending 转人工核对（fence 前进）：此后补到的 Sent 证据只记不推进。
    sqlx::query(
        "UPDATE public.deliveries
         SET status = 'manual_reconcile', lease_token = NULL, lease_expires_at = NULL,
             completed_at = clock_timestamp(), failure_code = 'lease_expired_outcome_unknown',
             fencing_version = fencing_version + 1
         WHERE id = $1 AND status = 'sending'",
    )
    .bind(seeded.delivery_id)
    .execute(pool)
    .await
    .expect("模拟过期回收");
    let before = delivery_row(pool, seeded.delivery_id).await;
    assert!(!seeded.service().record_evidence(sent(started.attempt)).await.expect("证据").replayed);
    assert_eq!(delivery_row(pool, seeded.delivery_id).await, before);
    assert_eq!(tallies(pool, seeded.delivery_id).await, (0, 1, 1));
}
