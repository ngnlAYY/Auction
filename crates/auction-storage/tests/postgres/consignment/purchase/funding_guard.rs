//! 资金守卫的显式寄售分支：寄售义务的收款入账与现金 Send 资金门一律拒绝，资金分录与外写数不变。

use auction_application::settlement::{
    FundingCollectedReceipt, FundingError, FundingMutation, INTERNAL_EVIDENCE_PROTOCOL,
    InternalEvidencePurpose, MailAccountEvidenceBinding, MailRuntimeBinding,
    SettlementInternalEvidenceSnapshot, VerifiedMailSentCollectionObservation,
};
use auction_domain::SettlementWorld;
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::dispatch_gate::purchased_and_leased;
use super::fixture::{Fixture, PRICE};

/// 夹具资金账户。
const UID: &str = "7001";

fn mutation(byte: u8) -> FundingMutation {
    FundingMutation { command_id: Uuid::from_bytes([byte; 16]), request_digest: [byte; 32] }
}

/// (资金分录, 资金分配, 投递资金绑定, 冻结 Send 尝试) 行数。
async fn money_and_send_rows(pool: &PgPool) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.settlement_funding_entries),
                (SELECT count(*) FROM public.settlement_funding_allocations),
                (SELECT count(*) FROM public.settlement_funding_delivery_bindings),
                (SELECT count(*) FROM public.delivery_send_attempts)",
    )
    .fetch_one(pool)
    .await
    .expect("资金与外写计数")
}

/// 已收款观察（明显的假值）：只为走到入账入口；寄售分支在任何资金写入之前拒绝。
fn collected_evidence(command: &FundingMutation) -> SettlementInternalEvidenceSnapshot {
    let observed = Timestamp::now().duration_trunc(Duration::microseconds(1)).expect("截断");
    let mut identity = MailAccountEvidenceBinding {
        credential_id: Uuid::from_bytes([0x80; 16]),
        credential_version: 1,
        runtime: MailRuntimeBinding { generation: 1 },
        authenticated_uid: UID.to_owned(),
        player_name: "Fake Bot".to_owned(),
        world: SettlementWorld::Main,
        observed_at: observed,
        protocol_version: INTERNAL_EVIDENCE_PROTOCOL,
        digest: [0; 32],
    };
    identity.digest = identity.compute_digest();
    let sent = VerifiedMailSentCollectionObservation {
        binding: identity.clone(),
        external_mail_id: "900001".to_owned(),
        recipient: "FakeSellerTwo".to_owned(),
        subject: "fake subject".to_owned(),
        sent_at: observed,
        observed_at: observed,
        remaining_credits: 0,
        cod_pending: 0,
        remaining_equipment_ids: Vec::new(),
        collected: true,
        digest: [0; 32],
    };
    let mut evidence = SettlementInternalEvidenceSnapshot {
        operation_id: command.command_id,
        purpose: InternalEvidencePurpose::FundingReconciliation {
            command_id: command.command_id,
            expected_account_version: 0,
        },
        identity,
        equipment: None,
        main_cash: None,
        sent_collection: Some(sent),
        observed_from: observed,
        expires_at: observed + Duration::seconds(60),
        digest: [0; 32],
    };
    evidence.digest = evidence.compute_digest();
    evidence
}

#[tokio::test]
async fn postgres_consignment_funding_reserve_rejects_a_consignment_delivery() {
    let fixture = Fixture::available().await;
    let (attempt, _token) = purchased_and_leased(&fixture).await;
    let before = money_and_send_rows(fixture.pool()).await;
    let error = fixture
        .runtime
        .test_reserve_funded_delivery(UID, &mutation(0x41), attempt.delivery_id)
        .await
        .expect_err("寄售投递不得进入通用现金资金门");
    assert!(matches!(error, FundingError::Unproven), "{error:?}");
    assert_eq!(money_and_send_rows(fixture.pool()).await, before);
}

#[tokio::test]
async fn postgres_consignment_funding_collected_payment_rejects_a_consignment_obligation() {
    let fixture = Fixture::available().await;
    let (attempt, _token) = purchased_and_leased(&fixture).await;
    let payment: Uuid = sqlx::query_scalar(
        "SELECT obligation_id FROM public.consignment_dispatch_rounds WHERE delivery_id = $1",
    )
    .bind(attempt.delivery_id)
    .fetch_one(fixture.pool())
    .await
    .expect("付款义务");
    let before = money_and_send_rows(fixture.pool()).await;
    let command = mutation(0x42);
    let evidence = collected_evidence(&command);
    let receipt = FundingCollectedReceipt {
        payment_obligation_id: payment,
        delivery_id: attempt.delivery_id,
        buyer_credits: PRICE,
        evidence: &evidence,
    };
    let error = fixture
        .runtime
        .test_fund_collected_payment(UID, &command, &receipt)
        .await
        .expect_err("寄售付款义务不得走拍卖收款入账");
    assert!(matches!(error, FundingError::Unproven), "{error:?}");
    assert_eq!(money_and_send_rows(fixture.pool()).await, before);
}
