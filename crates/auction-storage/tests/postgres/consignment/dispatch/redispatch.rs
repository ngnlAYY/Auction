//! 新轮（Redispatch）：只能紧接上一轮，并引用已封闭上一轮的退件接管与消费（同合同、同义务、同原轮）；
//! 冻结权利人必须是该义务的受益人；轮 ID 由轮键确定，重放不产生第二轮。
//! 授权命令 ID 由轮键派生：同一轮被确定拒绝后，异请求再授权按命令冲突失败关闭。
//!
//! 夹具复用第 6 期的 SQL：义务 3（买家退款腿）有首轮与一件已消费的 intake，义务 1 有首轮但没有 intake。

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, DispatchService,
};
use auction_domain::consignment::dispatch::{
    DispatchRoundKey, FrozenDispatchIdentity, RoundOrigin, dispatch_ref, dispatch_round_id,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_1, OBLIGATION_3};
use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::{Intake, original_dispatch_sql};
use super::super::ledger136::returns::{CONSUMPTION, consumption_sql, proof_sql, seed};
use super::fixture::{PRICE, frozen, uuid};
use crate::support::TestDatabase;

fn second_round(obligation: &str) -> (DispatchRoundKey, FrozenDispatchIdentity) {
    let key =
        DispatchRoundKey { contract_id: uuid(CONTRACT), obligation_id: uuid(obligation), round: 2 };
    let mut identity = frozen(PRICE);
    identity.round_ref = dispatch_ref(&key);
    (key, identity)
}

/// 新轮的投递：备好（`prepared`）、属于该义务、载荷带冻结身份摘要。
async fn prepare_delivery(
    pool: &PgPool,
    obligation: &str,
    identity: &FrozenDispatchIdentity,
) -> Uuid {
    let delivery = Uuid::new_v4();
    let digest: String =
        identity.identity_digest().iter().map(|byte| format!("{byte:02x}")).collect();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, created_at, updated_at)
         VALUES ($1, $1, decode(repeat('c6', 32), 'hex'), 'settlement', $2, 'FakeSellerTwo',
                 jsonb_build_object('identity_digest', $3::text), 'prepared', 0, 0,
                 '2026-01-01T04:00:00Z', '2026-01-01T04:00:00Z')",
    )
    .bind(delivery)
    .bind(obligation)
    .bind(digest)
    .execute(pool)
    .await
    .expect("备好新轮投递");
    delivery
}

/// 履约行的 (状态, 指向的投递, 状态版本)。
async fn fulfillment_of(pool: &PgPool, obligation: &str) -> (String, Option<Uuid>, i64) {
    sqlx::query_as(
        "SELECT execution_state, delivery_id, state_version
         FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1::uuid",
    )
    .bind(obligation)
    .fetch_one(pool)
    .await
    .expect("履约行")
}

async fn rounds_of(pool: &PgPool, obligation: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_dispatch_rounds WHERE obligation_id = $1::uuid",
    )
    .bind(obligation)
    .fetch_one(pool)
    .await
    .expect("读取轮数")
}

#[tokio::test]
async fn postgres_consignment_dispatch_redispatch_needs_the_closed_intake_of_the_same_obligation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    succeeds(pool, &original_dispatch_sql()).await;
    let intake = Intake { n: 1, leg: "cash_refund" };
    succeeds(pool, &intake.sql()).await;
    succeeds(pool, &intake.start_sql()).await;
    succeeds(pool, &(proof_sql(&intake) + &consumption_sql(&intake, CONSUMPTION))).await;
    let service = DispatchService::new(std::sync::Arc::new(runtime));
    let closed = RoundOrigin::Redispatch { closed_by_return_intake: uuid(&intake.intake()) };

    // 反例：intake 属于义务 3，不能为义务 1 开新轮（义务 1 有上一轮，投递也已备好）。
    let (other_key, other_identity) = second_round(OBLIGATION_1);
    let other_delivery = prepare_delivery(pool, OBLIGATION_1, &other_identity).await;
    let fulfillment_before = fulfillment_of(pool, OBLIGATION_1).await;
    let denied = service
        .authorize(other_key, closed, other_delivery, other_identity)
        .await
        .expect_err("别的义务的 intake 不能授权新轮");
    assert_eq!(denied, ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven));
    assert_eq!(rounds_of(pool, OBLIGATION_1).await, 1);
    // 上一轮未被本义务的退件消费封闭：履约行不重置、不改指。
    assert_eq!(fulfillment_of(pool, OBLIGATION_1).await, fulfillment_before);

    // 正例：同合同、同义务、原轮为 1 的已消费 intake 授权第 2 轮。
    let (key, identity) = second_round(OBLIGATION_3);
    let delivery = prepare_delivery(pool, OBLIGATION_3, &identity).await;
    let receipt =
        service.authorize(key, closed, delivery, identity.clone()).await.expect("授权新轮");
    assert_eq!((receipt.round, receipt.delivery_id, receipt.replayed), (2, delivery, false));
    // 轮 ID 由轮键确定（即补寄处置排队的 dispatch_id）；同命令重放回原回执，不建第二轮。
    assert_eq!(receipt.round_id, dispatch_round_id(&key));
    let replay = service.authorize(key, closed, delivery, identity).await.expect("重放");
    assert_eq!((replay.round_id, replay.replayed), (receipt.round_id, true));
    let stored: (i32, Uuid, Option<i32>, Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT round, delivery_id, prior_round, authorized_by_intake, authorized_by_consumption
         FROM public.consignment_dispatch_rounds
         WHERE obligation_id = $1::uuid AND round = 2",
    )
    .bind(OBLIGATION_3)
    .fetch_one(pool)
    .await
    .expect("第 2 轮已落库");
    assert_eq!(
        stored,
        (2, delivery, Some(1), Some(uuid(&intake.intake())), Some(uuid(CONSUMPTION)))
    );
    assert_eq!(rounds_of(pool, OBLIGATION_3).await, 2);
}

/// 补寄轮的冻结权利人必须是该义务的受益人（买家退款腿取 sale 冻结买方）；以卖家名义冻结的轮被拒，
/// 不建轮。投递已按该身份备好，只有受益人核对能挡住它。
#[tokio::test]
async fn postgres_consignment_dispatch_redispatch_rejects_a_holder_other_than_the_beneficiary() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    succeeds(pool, &original_dispatch_sql()).await;
    let intake = Intake { n: 1, leg: "cash_refund" };
    succeeds(pool, &intake.sql()).await;
    succeeds(pool, &intake.start_sql()).await;
    succeeds(pool, &(proof_sql(&intake) + &consumption_sql(&intake, CONSUMPTION))).await;
    let service = DispatchService::new(std::sync::Arc::new(runtime));
    let closed = RoundOrigin::Redispatch { closed_by_return_intake: uuid(&intake.intake()) };

    let (key, mut seller) = second_round(OBLIGATION_3);
    seller.rights_holder_identity = "FakeSellerOne".to_owned();
    let delivery = prepare_delivery(pool, OBLIGATION_3, &seller).await;
    let denied =
        service.authorize(key, closed, delivery, seller).await.expect_err("冻结权利人不是受益人");
    assert_eq!(denied, ConsignmentError::Deposit(DepositBusinessRejection::SourceMismatch));
    assert_eq!(rounds_of(pool, OBLIGATION_3).await, 1, "被拒的补寄不得建轮");
}
