//! 经真实购买路径产生的首轮：冻结的权利人是买方，衔接租约后起写授权通过；买方改名后保留权利、零外写。

use std::sync::Arc;

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, DispatchService, PurchaseOutcome, PurchaseStore,
    WriteStart,
};
use auction_application::delivery::DeliverySendCredentialBinding;
use auction_domain::consignment::dispatch::AttemptKey;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::deposit::support::CREDENTIAL;
use super::fixture::{BUYER, Fixture, purchase};

/// 起写必带的凭据绑定（夹具凭据版本 1、运行时 generation 1）。
fn binding() -> DeliverySendCredentialBinding {
    DeliverySendCredentialBinding {
        credential_id: Uuid::parse_str(CREDENTIAL).expect("凭据 ID"),
        credential_version: 1,
        runtime_generation: 1,
    }
}

/// 购买后领到首轮投递租约：返回 (尝试键, 投递租约 token)。
pub(super) async fn purchased_and_leased(fixture: &Fixture) -> (AttemptKey, Uuid) {
    let outcome = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("购买");
    let PurchaseOutcome::Purchased(receipt) = outcome else {
        panic!("应成交：{outcome:?}");
    };
    let (round_id, obligation_id, holder, recipient): (Uuid, Uuid, String, String) =
        sqlx::query_as(
            "SELECT round.round_id, round.obligation_id, round.rights_holder_identity,
                    delivery.recipient
             FROM public.consignment_dispatch_rounds AS round
             JOIN public.deliveries AS delivery ON delivery.id = round.delivery_id
             WHERE round.contract_id = $1 AND round.round = 1",
        )
        .bind(receipt.contract_id.as_uuid())
        .fetch_one(fixture.pool())
        .await
        .expect("首轮");
    // 首轮行使买方交割权：冻结的权利人就是收件的买方。
    assert_eq!(holder, recipient);
    // 起写锁验认 Worker 侧的凭据命名空间，并要求运行时 generation 已提交。
    sqlx::query("UPDATE public.credential_keys SET scope = 'mooglemail:main' WHERE id = $1::uuid")
        .bind(CREDENTIAL)
        .execute(fixture.pool())
        .await
        .expect("凭据 scope");
    sqlx::query(
        "INSERT INTO public.mail_runtime_generation (scope, generation, activated_at)
         VALUES ('mail_cookie', 1, clock_timestamp()) ON CONFLICT (scope) DO NOTHING",
    )
    .execute(fixture.pool())
    .await
    .expect("运行时 generation");
    let claim_token = Uuid::new_v4();
    let fencing: i64 = sqlx::query_scalar(
        "UPDATE public.settlement_obligation_fulfillments
         SET claim_token = $2, claimed_at = clock_timestamp(),
             claim_expires_at = clock_timestamp() + interval '5 minutes',
             claim_fencing = claim_fencing + 1, state_version = state_version + 1
         WHERE obligation_id = $1 RETURNING claim_fencing",
    )
    .bind(obligation_id)
    .bind(claim_token)
    .fetch_one(fixture.pool())
    .await
    .expect("模拟寄售义务领取");
    let (delivery_id, token, fence) = fixture
        .runtime
        .test_lease_dispatch_round(obligation_id, claim_token, fencing, round_id)
        .await
        .expect("衔接")
        .expect("取得投递租约");
    (AttemptKey { delivery_id, fence }, token)
}

async fn send_writes(pool: &PgPool, delivery: Uuid) -> (i64, i64, String) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.delivery_send_attempts WHERE delivery_id = $1),
                (SELECT count(*) FROM public.consignment_dispatch_attempts WHERE delivery_id = $1),
                (SELECT status FROM public.deliveries WHERE id = $1)",
    )
    .bind(delivery)
    .fetch_one(pool)
    .await
    .expect("外写面")
}

#[tokio::test]
async fn postgres_consignment_purchase_first_round_passes_the_send_entitlement() {
    let fixture = Fixture::available().await;
    let (attempt, token) = purchased_and_leased(&fixture).await;
    let service = DispatchService::new(Arc::new(fixture.runtime.clone()));
    let started =
        service.mark_write_started(attempt, token, Some(binding())).await.expect("起写授权通过");
    assert!(matches!(started, WriteStart::FirstCommitted { .. }), "{started:?}");
    assert_eq!(
        send_writes(fixture.pool(), attempt.delivery_id).await,
        (1, 1, "sending".to_owned())
    );
}

#[tokio::test]
async fn postgres_consignment_purchase_first_round_retains_a_renamed_buyer_with_zero_send_writes() {
    let fixture = Fixture::available().await;
    let (attempt, token) = purchased_and_leased(&fixture).await;
    sqlx::query("UPDATE public.users SET username = 'FakeRenamedBuyer' WHERE id = $1::uuid")
        .bind(BUYER)
        .execute(fixture.pool())
        .await
        .expect("买方改名");
    let service = DispatchService::new(Arc::new(fixture.runtime.clone()));
    let error = service
        .mark_write_started(attempt, token, Some(binding()))
        .await
        .expect_err("改名不得起写");
    assert_eq!(error, ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven));
    assert_eq!(
        send_writes(fixture.pool(), attempt.delivery_id).await,
        (0, 0, "claimed".to_owned())
    );
}
