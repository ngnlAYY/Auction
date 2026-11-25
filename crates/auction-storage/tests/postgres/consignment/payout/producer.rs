//! 下游首轮生产者与确领放行：恰建一轮（并发、重放）、前置缺一不建且起写被拒、退款转换后建一轮。
//!
//! 保护不变量 1、2、4、6：涉及外写的用例断言新增 POST 数与已提交的起写命令数。

use auction_application::consignment::{ConsignmentError, DepositBusinessRejection};
use auction_domain::consignment::dispatch::{AttemptKey, DispatchRoundKey, FrozenDispatchIdentity};
use auction_jobs::consignment::DispatchRound;
use auction_time::Timestamp;
use uuid::Uuid;

use super::super::acceptance::chain::{Order, World};
use super::super::contracts::fixture::{CONTRACT, OBLIGATION_3};
use super::super::dispatch::carrier_funding::{BALANCE, UID, cash_evidence};
use super::super::dispatch::cash_rounds::{claim_delivery, frozen, insert_round};
use super::super::dispatch::fixture::binding;
use super::super::ledger136::fixture::{Intake, REFUND_DELIVERY, REFUND_MID};
use super::super::returns::fixture::UNSIGNED_VALIDATOR;

/// 已购、首派已寄出、付款证据已记录（尚未实收认证）的 main 订单。
async fn main_paid_but_uncertified() -> Order {
    let order = Order::available(World::Main).await;
    order.purchase().await.expect("购买");
    assert_eq!(order.run_executor().await, DispatchRound::Sent);
    let attempt = order.first_round(order.obligation("winner_equipment_cod").await).await;
    order.record_payment(attempt).await;
    order
}

/// 本合同起写命令与远端尝试的条数（每次外写许可都先提交一条起写）。
async fn send_writes(order: &Order) -> (i64, i64) {
    (
        order.count("delivery_send_attempts").await,
        order
            .scalar(
                "SELECT count(*) FROM public.consignment_commands
                 WHERE operation = 'mark_write_started' AND result_status = 200"
                    .to_owned(),
            )
            .await,
    )
}

/// 正例：实收认证与原件确领都成立后卖款放行；两个并发生产者与重放只建一轮、一封投递，同一确领的并发重放
/// 只留一份确领证明。
#[tokio::test]
async fn postgres_consignment_payout_round_is_created_once_after_certify_and_handoff() {
    let order = main_paid_but_uncertified().await;
    let attempt = order.first_round(order.obligation("winner_equipment_cod").await).await;
    order.certify(attempt).await.expect("实收认证");

    let digest = [0xe1; 32];
    let (first, second) = tokio::join!(
        order.prove_handoff("winner_equipment_cod", "fake-cod-paid-1", digest),
        order.prove_handoff("winner_equipment_cod", "fake-cod-paid-1", digest),
    );
    let replays = [first.expect("确领"), second.expect("确领")].map(|receipt| receipt.replayed);
    assert_eq!(replays.iter().filter(|replayed| !**replayed).count(), 1, "恰一次首次提交");
    assert_eq!(order.count("consignment_handoff_proofs").await, 1);
    assert_eq!(order.business_state("seller_payout").await, "ready");

    let (left, right) = tokio::join!(order.produce_rounds(), order.produce_rounds());
    assert_eq!(left + right, 1, "两个并发生产者只建一轮");
    assert_eq!(order.produce_rounds().await, 0, "重放零新增");
    let payout = order.obligation("seller_payout").await;
    let rounds: i64 = order
        .scalar(format!(
            "SELECT count(*) FROM public.consignment_dispatch_rounds WHERE obligation_id = '{payout}'"
        ))
        .await;
    let deliveries: i64 = order
        .scalar(format!("SELECT count(*) FROM public.deliveries WHERE origin_id = '{payout}'"))
        .await;
    assert_eq!((rounds, deliveries), (1, 1));
}

/// 反例：前置缺一不放行——实收未认证时确领被拒（不占命令、零证明零放行），认证后未确领则生产者不建卖款轮；
/// 以 SQL 硬造的卖款轮起写被确定拒绝：零许可、零远端尝试、S 原样在账。
#[tokio::test]
async fn postgres_consignment_payout_prerequisite_missing_blocks_round_and_write_start() {
    let order = main_paid_but_uncertified().await;
    let denied = order.prove_handoff("winner_equipment_cod", "fake-cod-paid-1", [0xe1; 32]).await;
    assert_eq!(
        denied.expect_err("实收未认证"),
        ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven)
    );
    assert_eq!(order.count("consignment_handoff_proofs").await, 0);
    let ledgered: i64 = order
        .scalar(
            "SELECT count(*) FROM public.consignment_commands WHERE operation = 'prove_handoff'"
                .to_owned(),
        )
        .await;
    assert_eq!(ledgered, 0, "临时缺证不占命令 ID");

    let attempt = order.first_round(order.obligation("winner_equipment_cod").await).await;
    order.certify(attempt).await.expect("实收认证");
    assert_eq!(order.business_state("seller_payout").await, "blocked");
    assert_eq!(order.produce_rounds().await, 0, "未确领原件不建卖款轮");

    // 硬造的卖款轮：义务仍 blocked，起写复核失败关闭。
    let seller_amount: i64 =
        order.scalar("SELECT seller_amount FROM public.consignment_sales".to_owned()).await;
    let key = DispatchRoundKey {
        contract_id: order.contract_id().await,
        obligation_id: order.obligation("seller_payout").await,
        round: 1,
    };
    let seller = FrozenDispatchIdentity {
        rights_holder_identity: "FakeSellerOne".to_owned(),
        ..frozen(&key, "consignment_seller_payout", seller_amount, 0)
    };
    let delivery = insert_round(order.pool(), &key, &seller, "FakeSellerOne", None).await;
    let token = Uuid::new_v4();
    claim_delivery(order.pool(), delivery, token).await;
    let forged = AttemptKey { delivery_id: delivery, fence: 1 };
    order.service.start_attempt(key, forged, token, seller).await.expect("登记尝试");
    let before = send_writes(&order).await;
    let evidence = cash_evidence(
        order.pool(),
        forged,
        token,
        &binding(),
        UID,
        Some(BALANCE),
        Timestamp::now(),
    )
    .await;
    let started =
        order.service.mark_cash_write_started(forged, token, Some(binding()), &evidence).await;
    assert_eq!(
        started.expect_err("卖款未放行"),
        ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed)
    );
    assert_eq!(send_writes(&order).await, before, "零许可、零远端尝试");
    assert_eq!(order.allocation("seller").await, Some(("available".to_owned(), seller_amount)));
}

/// 退款转换后恰建一轮：并发转换只转一次、只激活一次，两个并发生产者只建一轮；转换前生产者不建退款轮。
#[tokio::test]
async fn postgres_consignment_refund_round_is_created_once_after_conversion() {
    let order = Order::available(World::Isekai).await;
    order.purchase().await.expect("购买");
    assert_eq!(order.run_executor().await, DispatchRound::Sent);
    let attempt = order.first_round(order.obligation("winner_payment_carrier_cod").await).await;
    order.record_payment(attempt).await;
    order.certify(attempt).await.expect("实收认证");
    assert_eq!(order.business_state("buyer_refund").await, "contingent");

    let (first, second) = tokio::join!(order.convert_refund(), order.convert_refund());
    let replays = [first.expect("转换"), second.expect("转换")].map(|receipt| receipt.replayed);
    assert_eq!(replays.iter().filter(|replayed| !**replayed).count(), 1);
    assert_eq!(order.business_state("buyer_refund").await, "ready");
    assert_eq!(order.allocation("refund").await, Some(("available".to_owned(), 101)));

    let (left, right) = tokio::join!(order.produce_rounds(), order.produce_rounds());
    assert_eq!(left + right, 1, "两个并发生产者只建一轮（交割已随退款不再相符）");
    assert_eq!(order.produce_rounds().await, 0);
    let refund = order.obligation("buyer_refund").await;
    let handoff = order.obligation("winner_equipment_handoff").await;
    let rounds: (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE obligation_id = $1), count(*) FILTER (WHERE obligation_id = $2)
         FROM public.consignment_dispatch_rounds",
    )
    .bind(refund)
    .bind(handoff)
    .fetch_one(order.pool())
    .await
    .expect("轮数");
    assert_eq!(rounds, (1, 0));
}

/// 确领前后都应不变的业务事实：(确领证明数, position, 挂单, funded 处置, 卖款业务态)。
async fn handoff_facts(order: &Order) -> (i64, String, String, String, String) {
    (
        order.count("consignment_handoff_proofs").await,
        order.position_state().await,
        order.scalar("SELECT state FROM public.consignment_listings".to_owned()).await,
        order.scalar("SELECT disposition FROM public.consignment_funded_sales".to_owned()).await,
        order.business_state("seller_payout").await,
    )
}

/// 已购、首派已寄出、实收已认证、尚未确领原件的 main 订单。
async fn main_certified() -> Order {
    let order = main_paid_but_uncertified().await;
    let attempt = order.first_round(order.obligation("winner_equipment_cod").await).await;
    order.certify(attempt).await.expect("实收认证");
    order
}

/// 反例：未签收验证器产出的原件确领观察不成为结论——确领被拒且不占命令，零证明，position、挂单、funded 与
/// 卖款均不变；随后已签收的证据照常放行（命令 ID 未被占死）。
#[tokio::test]
async fn postgres_consignment_payout_unsigned_original_claim_is_refused() {
    let order = main_certified().await;
    let attempt = order.first_round(order.obligation("winner_equipment_cod").await).await;
    order
        .record_original_claim_by(attempt, "fake-original-claim-unsigned", UNSIGNED_VALIDATOR)
        .await;
    let before = handoff_facts(&order).await;
    assert_eq!(
        before,
        (
            0,
            "in_transit".to_owned(),
            "sale_pending".to_owned(),
            "funded".to_owned(),
            "blocked".to_owned()
        )
    );

    let denied = order
        .prove_handoff("winner_equipment_cod", "fake-original-claim-unsigned", [0xe2; 32])
        .await;
    assert_eq!(
        denied.expect_err("验证器未签收"),
        ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven)
    );
    assert_eq!(handoff_facts(&order).await, before);

    // 已签收的同轮证据仍可确领：未签收的那次没有写账本。
    let proven = order.prove_handoff("winner_equipment_cod", "fake-cod-paid-1", [0xe1; 32]).await;
    assert!(!proven.expect("已签收").replayed);
    assert_eq!(order.business_state("seller_payout").await, "ready");
}

/// 反例：同轮原件已有退件接管（可信 RTS 已被接管）时确领与之互斥，确定冲突；零证明，position、挂单、funded 与
/// 卖款均不变。
#[tokio::test]
async fn postgres_consignment_payout_handoff_conflicts_with_original_return_intake() {
    let order = main_certified().await;
    let original = order.obligation("winner_equipment_cod").await;
    let attempt = order.first_round(original).await;
    let sent_mid: String = order
        .scalar(format!(
            "SELECT external_mail_id FROM public.delivery_sent_bindings
             WHERE delivery_id = '{}'",
            attempt.delivery_id
        ))
        .await;
    // 同轮原件腿的退件接管：沿用接管夹具的整条事实（原信、观察、终局处置、Take 工作与投递），换成本订单的
    // 合同、原件义务、原轮投递与 Sent MID。
    let intake = Intake { n: 41, leg: "original" }
        .sql()
        .replace(CONTRACT, &order.contract_id().await.to_string())
        .replace(OBLIGATION_3, &original.to_string())
        .replace(REFUND_DELIVERY, &attempt.delivery_id.to_string())
        .replace(&format!("'{REFUND_MID}'"), &format!("'{sent_mid}'"));
    sqlx::raw_sql(sqlx::AssertSqlSafe(intake)).execute(order.pool()).await.expect("预置退件接管");
    let before = handoff_facts(&order).await;

    let denied = order.prove_handoff("winner_equipment_cod", "fake-cod-paid-1", [0xe1; 32]).await;
    assert_eq!(
        denied.expect_err("原件已退回"),
        ConsignmentError::Deposit(DepositBusinessRejection::EvidenceConflict)
    );
    assert_eq!(handoff_facts(&order).await, before);
    assert_eq!(before.4, "blocked");
}
