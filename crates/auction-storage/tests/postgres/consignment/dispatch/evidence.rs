//! 证据：只追加、同键异摘要冲突；Sent MID 与拍卖已绑 MID 跨来源冲突。

use auction_application::consignment::RecordEvidenceInput;
use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, DispatchStore, RecordEvidence,
};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use auction_domain::consignment::dispatch::EvidenceCategory;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::OBLIGATION_2;
use super::super::deposit::support::{fails, succeeds};
use super::fixture::{Seeded, at};

const AUCTION_MID: &str = "9100";
const OWN_MID: &str = "9001";
const AUCTION_DELIVERY: &str = "c9100000-0000-4000-8000-000000000001";

/// 拍卖侧一次真实 Send：投递、冻结尝试与已绑定的 Sent MID。
fn auction_send_sql() -> String {
    format!(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, created_at, updated_at)
         VALUES ('{AUCTION_DELIVERY}', 'c9100000-0000-4000-8000-000000000002',
                 decode(repeat('c1', 32), 'hex'), 'settlement', '{OBLIGATION_2}',
                 'FakeSellerTwo', '{{}}'::jsonb, 'prepared', 0, 0,
                 '2026-01-01T04:00:00Z', '2026-01-01T04:00:00Z');
         INSERT INTO public.delivery_send_attempts
            (delivery_id, fencing_version, command_id, request_digest, world_id, sending_at,
             expectation)
         VALUES ('{AUCTION_DELIVERY}', 1, 'c9100000-0000-4000-8000-000000000002',
                 decode(repeat('c2', 32), 'hex'), 'main', '2026-01-01T04:00:00Z',
                 jsonb_build_object('world_id', 'main', 'recipient', 'FakeSellerTwo',
                     'subject', 'fake', 'body_digest',
                     (SELECT jsonb_agg(0) FROM generate_series(1, 32)), 'credits', 0,
                     'cod', 0, 'equipment_ids', '[]'::jsonb, 'credential', 'null'::jsonb));
         INSERT INTO public.delivery_sent_bindings
            (world_id, external_mail_id, delivery_id, fencing_version,
             expectation_fencing_version)
         VALUES ('main', '{AUCTION_MID}', '{AUCTION_DELIVERY}', 1, 1);"
    )
}

async fn evidence_rows(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM public.consignment_dispatch_evidence")
        .fetch_one(pool)
        .await
        .expect("证据行数")
}

fn input(
    started: &super::fixture::Started,
    category: EvidenceCategory,
    observation: &str,
    digest: u8,
) -> RecordEvidenceInput {
    RecordEvidenceInput {
        attempt: started.attempt,
        category,
        remote_observation_id: observation.to_owned(),
        observed_at: at("2026-01-01T05:00:00Z"),
        validator_version: "fake-validator-v1".to_owned(),
        evidence_digest: [digest; 32],
    }
}

#[tokio::test]
async fn postgres_consignment_dispatch_sent_mid_cannot_be_shared_with_an_auction_delivery() {
    let (seeded, started) = Seeded::first_dispatch_started().await;
    let pool = seeded.pool();
    succeeds(pool, &auction_send_sql()).await;
    let service = seeded.service();
    // 反例：该 MID 已绑定到拍卖投递，寄售尝试不能再绑，也不留证据行或绑定。
    let denied = service
        .record_evidence(input(&started, EvidenceCategory::SentBinding, AUCTION_MID, 1))
        .await
        .expect_err("跨来源 MID 必须冲突");
    assert_eq!(denied, ConsignmentError::Deposit(DepositBusinessRejection::EvidenceConflict));
    assert_eq!(evidence_rows(pool).await, 0);
    let stolen: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.delivery_sent_bindings WHERE delivery_id = $1",
    )
    .bind(seeded.delivery_id)
    .fetch_one(pool)
    .await
    .expect("绑定数");
    assert_eq!(stolen, 0);
    // 正例：未被占用的 MID 绑定成功，绑定指向本投递与起写 fence；重放不产生第二条事实。
    let own = input(&started, EvidenceCategory::SentBinding, OWN_MID, 2);
    assert!(!service.record_evidence(own.clone()).await.expect("绑定").replayed);
    assert!(service.record_evidence(own).await.expect("重放").replayed);
    let bound: (Uuid, i64) = sqlx::query_as(
        "SELECT delivery_id, fencing_version FROM public.delivery_sent_bindings
         WHERE world_id = 'main' AND external_mail_id = $1",
    )
    .bind(OWN_MID)
    .fetch_one(pool)
    .await
    .expect("本投递的绑定");
    assert_eq!(bound, (seeded.delivery_id, 1));
    assert_eq!(evidence_rows(pool).await, 1);
}

#[tokio::test]
async fn postgres_consignment_dispatch_evidence_is_append_only_and_conflicts_on_changed_digest() {
    let (seeded, started) = Seeded::first_dispatch_started().await;
    let pool = seeded.pool();
    let command = |byte: u8, digest: u8| RecordEvidence {
        command_id: CommandId::from_uuid(Uuid::from_u128(
            0x5200_0000_0000_5000_8000_0000_0000_0000 | u128::from(byte),
        )),
        request_hash: RequestHash::new([byte; 32]),
        attempt: started.attempt,
        category: EvidenceCategory::OriginalClaim,
        remote_observation_id: "claim-1".to_owned(),
        observed_at: at("2026-01-01T05:00:00Z"),
        validator_version: "fake-validator-v1".to_owned(),
        evidence_digest: [digest; 32],
    };
    assert!(!seeded.runtime.record_evidence(command(1, 1)).await.expect("追加").replayed);
    // 同键同摘要（换命令 ID）是重放，不增行。
    assert!(seeded.runtime.record_evidence(command(2, 1)).await.expect("同摘要重放").replayed);
    assert_eq!(evidence_rows(pool).await, 1);
    // 同键异摘要是冲突，旧证据不被改写。
    let conflict = seeded.runtime.record_evidence(command(3, 2)).await.expect_err("异摘要");
    assert_eq!(conflict, ConsignmentError::Deposit(DepositBusinessRejection::EvidenceConflict));
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT digest FROM public.consignment_dispatch_evidence")
            .fetch_one(pool)
            .await
            .expect("证据摘要");
    assert_eq!(stored, vec![1; 32]);
    // 运行账号对证据只有 SELECT / INSERT：不能改、不能删。
    let runtime_pool = seeded.runtime.postgres_pool();
    fails(
        runtime_pool,
        "UPDATE public.consignment_dispatch_evidence SET digest = digest;",
        "42501",
        None,
    )
    .await;
    fails(runtime_pool, "DELETE FROM public.consignment_dispatch_evidence;", "42501", None).await;
}
