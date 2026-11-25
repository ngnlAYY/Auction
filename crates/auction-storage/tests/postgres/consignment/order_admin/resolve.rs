//! resolve 的退款类处置：只经 6B 单写者转换一次；相反付款未了结时零资金痕迹；证据不足与未实现动作失败关闭。

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, OrderResolveAction, OrderResolveReceipt,
};
use auction_storage::DatabaseStore;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::CONTRACT;
use super::super::contracts::fixture::OBLIGATION_2;
use super::super::deposit::support::succeeds;
use super::super::funding::fixture::{UID, entries_of};
use super::super::ledger136::fixture::{round_sql, send_sql};
use super::super::recovery::sale_recovery::refund_call::{
    PAYOUT_ATTEMPT, PAYOUT_DELIVERY, PAYOUT_MID, PAYOUT_ROUND, allocation, closures, funded_sale,
};
use super::super::recovery::{PROOF, command_ids, proof_sql};
use super::{command, count, ledger_of, rejection, resolve_input, service};

pub(super) fn contract() -> Uuid {
    Uuid::parse_str(CONTRACT).expect("合同")
}

pub(super) fn proof() -> Uuid {
    Uuid::parse_str(PROOF).expect("证据")
}

/// 已收款成交加一份属于该合同 position 的灭失证据；返回库、维护池与运行账号门面。
pub(super) async fn paid_sale_with_loss_proof()
-> (crate::support::TestDatabase, PgPool, DatabaseStore) {
    let (database, pool, store) = funded_sale().await;
    let commands = command_ids(&pool, 1).await;
    succeeds(&pool, &proof_sql(PROOF, "fake-observation-1", &commands[0])).await;
    drop(store);
    let runtime = database.store().await.expect("以运行账号连接");
    (database, pool, runtime)
}

pub(super) async fn funds(pool: &PgPool) -> (i64, i64, i64) {
    (
        allocation(pool, "refund").await,
        allocation(pool, "seller").await,
        allocation(pool, "fee").await,
    )
}

pub(super) async fn order_events(pool: &PgPool) -> i64 {
    count(
        pool,
        "SELECT count(*) FROM public.realtime_events WHERE stream_key LIKE 'consignment_order:%'",
    )
    .await
}

pub(super) async fn resolutions(pool: &PgPool) -> i64 {
    count(pool, "SELECT count(*) FROM public.consignment_order_resolutions").await
}

/// 正例：退款类处置经 6B 单写者全额转换（S+F=P），同事务落处置记录、订单阶段事件与回执；同命令重放
/// 返回原回执且不增分录、封闭事实与处置。反例：同命令 ID 换证据是异请求，冲突。
#[tokio::test]
async fn postgres_consignment_order_resolve_refund_converts_once_through_the_funding_writer() {
    let (_database, pool, runtime) = paid_sale_with_loss_proof().await;
    let service = service(runtime);

    let receipt = service
        .resolve(resolve_input(1, contract(), 1, OrderResolveAction::RefundLost, proof()))
        .await
        .expect("退款处置");

    let OrderResolveReceipt::ResolutionRecorded { contract_version, action, .. } = receipt else {
        panic!("应为 resolution_recorded：{receipt:?}");
    };
    assert_eq!(action, OrderResolveAction::RefundLost);
    assert_eq!(contract_version, 1, "回执版本是落账时订单阶段事件流的头部");
    assert_eq!(order_events(&pool).await, 1, "退款阶段变化同事务追加了一条订单阶段事件");
    assert_eq!(funds(&pool).await, (101, 0, 0), "S+F 全额转入退款");
    assert_eq!(closures(&pool).await, 1);
    assert_eq!(resolutions(&pool).await, 1);
    assert_eq!(ledger_of(&pool, command(1)).await, Some((200, "ok".to_owned())));
    let transfers = entries_of(&pool, UID, "transfer").await;

    let again = service
        .resolve(resolve_input(1, contract(), 1, OrderResolveAction::RefundLost, proof()))
        .await
        .expect("重放");
    assert_eq!(again, receipt, "同命令同请求返回原回执（含原版本）");
    assert_eq!(entries_of(&pool, UID, "transfer").await, transfers);
    assert_eq!((closures(&pool).await, resolutions(&pool).await), (1, 1));

    let other = service
        .resolve(resolve_input(
            1,
            contract(),
            1,
            OrderResolveAction::RefundLost,
            Uuid::from_u128(9),
        ))
        .await
        .expect_err("同 ID 异请求");
    assert!(matches!(other, ConsignmentError::CommandIdConflict));
    // 另一条命令想再退一次：已有退款处置，确定拒绝，资金不再变化。
    let second = service
        .resolve(resolve_input(2, contract(), 1, OrderResolveAction::RefundLost, proof()))
        .await
        .expect_err("第二次退款");
    assert_eq!(rejection(&second), Some(DepositBusinessRejection::DispositionConflict));
    assert_eq!(funds(&pool).await, (101, 0, 0));
    assert_eq!(order_events(&pool).await, 1, "重放与拒绝不再追加事件");
}

/// 反例：卖款腿已有 Send 尝试（卖款可能已发）→ `opposite_payment_open`：6B 写者不被调用，分配、分录、
/// 封闭事实与处置记录全部不变；确定拒绝入账，重放得到同一拒绝。
#[tokio::test]
async fn postgres_consignment_order_resolve_refuses_while_the_opposite_payment_is_open() {
    let (_database, pool, runtime) = paid_sale_with_loss_proof().await;
    succeeds(
        &pool,
        &(send_sql(PAYOUT_DELIVERY, OBLIGATION_2, PAYOUT_MID, 0)
            + &round_sql(
                PAYOUT_ROUND,
                PAYOUT_DELIVERY,
                OBLIGATION_2,
                PAYOUT_ATTEMPT,
                "cash_payout",
            )),
    )
    .await;
    let transfers = entries_of(&pool, UID, "transfer").await;
    let service = service(runtime);

    let error = service
        .resolve(resolve_input(3, contract(), 1, OrderResolveAction::RefundLost, proof()))
        .await
        .expect_err("相反付款未决");

    assert_eq!(rejection(&error), Some(DepositBusinessRejection::OppositePaymentOpen));
    assert_eq!(funds(&pool).await, (0, 96, 5), "卖款与手续费原样在账");
    assert_eq!(entries_of(&pool, UID, "transfer").await, transfers);
    assert_eq!((closures(&pool).await, resolutions(&pool).await), (0, 0));
    assert_eq!(order_events(&pool).await, 0, "拒绝不追加订单阶段事件");
    assert_eq!(ledger_of(&pool, command(3)).await, Some((409, "opposite_payment_open".to_owned())));
    let replay = service
        .resolve(resolve_input(3, contract(), 1, OrderResolveAction::RefundLost, proof()))
        .await
        .expect_err("重放原拒绝");
    assert_eq!(rejection(&error), rejection(&replay));
}

/// 反例：证据不存在（五个有写者的资金类动作各自的证据判据）全部确定拒绝，零资金痕迹；
/// 版本不符先报 `stale_version`。属于别的合同的真实证据见 `evidence` 模块。
#[tokio::test]
async fn postgres_consignment_order_resolve_fails_closed_without_evidence_or_a_writer() {
    let (_database, pool, runtime) = paid_sale_with_loss_proof().await;
    let service = service(runtime);
    let stranger = Uuid::from_u128(0x77);
    let cases: [(u128, OrderResolveAction, DepositBusinessRejection); 5] = [
        (4, OrderResolveAction::RefundLost, DepositBusinessRejection::EvidenceUnproven),
        (5, OrderResolveAction::RefundReturned, DepositBusinessRejection::EvidenceUnproven),
        (6, OrderResolveAction::RefundSeason, DepositBusinessRejection::EvidenceUnproven),
        (7, OrderResolveAction::CancelUnpaid, DepositBusinessRejection::EvidenceUnproven),
        (8, OrderResolveAction::RetainEntitlement, DepositBusinessRejection::EvidenceUnproven),
    ];
    for (n, action, expected) in cases {
        let error = service
            .resolve(resolve_input(n, contract(), 1, action, stranger))
            .await
            .expect_err("应确定拒绝");
        assert_eq!(rejection(&error), Some(expected), "{action:?}");
    }
    let stale = service
        .resolve(resolve_input(9, contract(), 7, OrderResolveAction::RefundLost, proof()))
        .await
        .expect_err("版本不符");
    assert_eq!(rejection(&stale), Some(DepositBusinessRejection::StaleVersion));
    assert_eq!(funds(&pool).await, (0, 96, 5), "没有任何资金变化");
    assert_eq!((closures(&pool).await, resolutions(&pool).await), (0, 0));
    // 不存在的合同：404，不入账。
    let missing = service
        .resolve(resolve_input(
            10,
            Uuid::from_u128(0x99),
            1,
            OrderResolveAction::RefundLost,
            proof(),
        ))
        .await
        .expect_err("合同不存在");
    assert!(matches!(missing, ConsignmentError::Business(_)));
    assert_eq!(ledger_of(&pool, command(10)).await, None);
}
