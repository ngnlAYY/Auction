//! 7C-4b 当前代权责分流的反例：当前代仍有买方权责 → 拒绝并改走原 Sale resolve；无法查清（含相反未知外写、
//! 仍可派发的原件腿工作）→ 保留占用、不迁移状态，按 `evidence_conflict` 记账本并写核对信号；已付款的开放
//! Sale → 拒绝；证据不属于这件实物 / 版本落后 → 确定拒绝。每个反例都断言零痕迹。
//!
//! 保护的不变量：2（锁内复验版本与当前代）、4（相反未知外写时不终局）、6（失败关闭，不静默修正）。

use auction_application::consignment::{
    DepositBusinessRejection, LossCaseAdmin, PositionLossStore,
};
use uuid::Uuid;

use super::super::super::contracts::fixture::{CONTRACT, OBLIGATION_1};
use super::super::super::deposit::support::{POSITION, succeeds};
use super::super::super::ledger136::fixture::{PAY_DELIVERY, PRICE, PURCHASED_AT, REFUND_DELIVERY};
use super::super::sale_recovery::refund_call::fund_open_sale;
use super::super::sale_recovery::{Closure, World, id, unknown_refund_sql};
use super::{admin_command_id, buyer_side_snapshot, confirm, record_proof, rejection};

/// 命令账本里的行数（拒绝回执会增加一行；回滚不增加）。
async fn ledger_rows(world: &World) -> i64 {
    world.count("consignment_commands").await
}

/// 断言灭失没有发生：position、挂单、占用与损失表原样。
async fn assert_still_held(
    world: &World,
    position: (String, Option<i64>, i64),
    listing: (String, i64),
) {
    assert_eq!(world.position().await, position);
    assert_eq!(world.listing().await, listing);
    assert_eq!(world.count("consignment_loss_facts").await, 0);
    assert_eq!(world.count("consignment_loss_cases").await, 0);
    let held: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_equipment_claims WHERE position_id IS NOT NULL",
    )
    .fetch_one(&world.pool)
    .await
    .expect("读取占用");
    assert_eq!(held, 1);
}

/// 反例：当前代的 Sale 未封闭（买方交割权仍有效）。不走 position 终局，确定拒绝并记安全回执；
/// 位置、挂单、占用与买方侧事实原样，没有任何损失行。同命令重放返回同一拒绝。
#[tokio::test]
async fn postgres_consignment_loss_confirm_refuses_while_the_current_sale_is_open() {
    let world = World::sold(Closure::None).await;
    let proof_id = record_proof(&world, 1).await;
    let buyer_before = buyer_side_snapshot(&world).await;
    let position = world.position().await;
    let listing = world.listing().await;

    let refused = world
        .deposit
        .confirm_position_loss(confirm(1, 3, proof_id, 0x41))
        .await
        .expect_err("买方权责仍在，不得按 position 办结");

    assert_eq!(rejection(refused), DepositBusinessRejection::ActionNotAllowed);
    assert_still_held(&world, position.clone(), listing.clone()).await;
    assert_eq!(buyer_side_snapshot(&world).await, buyer_before);
    let replay =
        world.deposit.confirm_position_loss(confirm(1, 3, proof_id, 0x41)).await.expect_err("重放");
    assert_eq!(rejection(replay), DepositBusinessRejection::ActionNotAllowed);
    assert_still_held(&world, position, listing).await;
}

/// 运维告警信号（核对案）的当前活动行数与累计计数。
async fn reconcile_signals(world: &World) -> (i64, i64) {
    sqlx::query_as(
        "SELECT count(*), COALESCE(sum(count_value), 0)::bigint
         FROM public.operations_alert_signals
         WHERE kind = 'recovery_manual_reconcile' AND active",
    )
    .fetch_one(&world.pool)
    .await
    .expect("读取告警信号")
}

async fn reconcile_rows(world: &World) -> i64 {
    world.count("consignment_loss_reconciles").await
}

/// 反例 + 补证后的正例：无法查清——同合同的买家退款派发已起写而结果未知（相反未知外写）。
/// 保留占用与双方责任、不做任何状态迁移：命令按确定拒绝 `evidence_conflict` 写入账本与审计（含私有
/// reason），同事务写一条运维告警信号作为核对案；同 ID 重放回原拒绝且不再累加信号；补证（该派发有了
/// Sent 绑定）后，新命令可办结。
#[tokio::test]
async fn postgres_consignment_loss_confirm_keeps_occupancy_and_raises_a_signal_when_unknown() {
    let world = World::sold(Closure::CancelledUnpaid).await;
    let proof_id = record_proof(&world, 1).await;
    succeeds(&world.owner, &unknown_refund_sql()).await;
    let buyer_before = buyer_side_snapshot(&world).await;
    let position = world.position().await;
    let listing = world.listing().await;
    let ledger_before = ledger_rows(&world).await;
    assert_eq!(reconcile_signals(&world).await, (0, 0));

    let kept = world
        .deposit
        .confirm_position_loss(confirm(1, 3, proof_id, 0x41))
        .await
        .expect_err("相反外写未知，不得终局");

    assert_eq!(rejection(kept), DepositBusinessRejection::EvidenceConflict);
    assert_still_held(&world, position.clone(), listing.clone()).await;
    assert_eq!(ledger_rows(&world).await, ledger_before + 1);
    assert_eq!(buyer_side_snapshot(&world).await, buyer_before);
    assert_eq!(reconcile_signals(&world).await, (1, 1));
    assert_eq!(reconcile_rows(&world).await, 1);
    let audited: Option<String> = sqlx::query_scalar(
        "SELECT metadata->>'loss_reason' FROM public.audit_records
         WHERE entity_id = $1 AND action = 'consignment.confirm_position_loss'",
    )
    .bind(admin_command_id(1).as_uuid().to_string())
    .fetch_one(&world.pool)
    .await
    .expect("读取审计");
    assert_eq!(audited.as_deref(), Some("fake loss reason"));

    // 同 ID 重放：回原拒绝，不增账本行、不再累加信号。
    let replay =
        world.deposit.confirm_position_loss(confirm(1, 3, proof_id, 0x41)).await.expect_err("重放");
    assert_eq!(rejection(replay), DepositBusinessRejection::EvidenceConflict);
    assert_eq!(ledger_rows(&world).await, ledger_before + 1);
    assert_eq!(reconcile_signals(&world).await, (1, 1));
    assert_eq!(reconcile_rows(&world).await, 1, "重放不增核对记录");
    let open = world.deposit.store().open_loss_reconciles(id(POSITION)).await.expect("列核对案");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].command_id, admin_command_id(1).as_uuid());

    // 同一 position 再一次无法查清（新命令）：核对记录按次累加，信号被收口后也不影响核对案。
    succeeds(&world.owner, "DELETE FROM public.operations_alert_signals;").await;
    let again = world
        .deposit
        .confirm_position_loss(confirm(3, 3, proof_id, 0x43))
        .await
        .expect_err("仍无法查清");
    assert_eq!(rejection(again), DepositBusinessRejection::EvidenceConflict);
    assert_eq!(reconcile_rows(&world).await, 2);
    assert_eq!(
        world.deposit.store().open_loss_reconciles(id(POSITION)).await.expect("列核对案").len(),
        2
    );

    // 补证：未知的退款派发有了 Sent 绑定；新命令可办结。
    succeeds(
        &world.owner,
        &format!(
            "INSERT INTO public.delivery_sent_bindings
                (world_id, external_mail_id, delivery_id, fencing_version,
                 expectation_fencing_version)
             VALUES ('main', '9001', '{REFUND_DELIVERY}', 1, 1);"
        ),
    )
    .await;
    world.deposit.confirm_position_loss(confirm(2, 3, proof_id, 0x42)).await.expect("补证后可办结");
    assert_eq!(world.position().await.0, "lost");
    assert_eq!(world.listing().await.0, "closed");
    // 办结后核对案不再算未结；历史记录仍在。
    assert!(world.deposit.store().open_loss_reconciles(id(POSITION)).await.expect("列").is_empty());
    assert_eq!(reconcile_rows(&world).await, 2);
}

/// 反例：Sale 已封闭，但原件腿的派发轮其投递仍未起写也未终结（相反可派工作）。不终局，按
/// `evidence_conflict` 拒绝并写核对信号，位置、挂单、占用原样；终止该投递属于投递 owner，本命令不代办。
#[tokio::test]
async fn postgres_consignment_loss_confirm_refuses_while_original_work_is_still_dispatchable() {
    let world = World::sold(Closure::CancelledUnpaid).await;
    let proof_id = record_proof(&world, 1).await;
    succeeds(
        &world.owner,
        &format!(
            "INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, fencing_version, attempt_count, created_at, updated_at)
             VALUES ('{PAY_DELIVERY}', 'c71{tail}', decode(repeat('c1', 32), 'hex'), 'settlement',
                     '{OBLIGATION_1}', 'FakeSellerTwo', '{{}}'::jsonb, 'prepared', 0, 0,
                     '{PURCHASED_AT}', '{PURCHASED_AT}');
             INSERT INTO public.consignment_dispatch_rounds
                (contract_id, obligation_id, round, round_id, delivery_id, rights_holder_identity,
                 bot_uid, world_id, subject, template, body_digest, round_ref, d0, credits, cod,
                 prior_round, authorized_by_intake, authorized_by_consumption)
             VALUES ('{CONTRACT}', '{OBLIGATION_1}', 1, 'c6600000-0000-4000-8000-000000000001',
                     '{PAY_DELIVERY}', 'FakeSellerTwo', '12345', 'main', 'fake subject',
                     'consignment_cod', decode(repeat('d1', 32), 'hex'),
                     'c6600000-0000-4000-8000-000000000001', '{PURCHASED_AT}', 0, {PRICE}, NULL,
                     NULL, NULL);",
            tail = &PAY_DELIVERY[3..],
        ),
    )
    .await;
    let position = world.position().await;
    let listing = world.listing().await;

    let kept = world
        .deposit
        .confirm_position_loss(confirm(1, 3, proof_id, 0x41))
        .await
        .expect_err("相反可派工作仍在，不得终局");

    assert_eq!(rejection(kept), DepositBusinessRejection::EvidenceConflict);
    assert_still_held(&world, position, listing).await;
    assert_eq!(reconcile_signals(&world).await, (1, 1));
}

/// 资金反例：当前代已收款（funded）且 Sale 仍开放——买家资金责任仍在。不得按 position 办结，
/// 确定拒绝 `action_not_allowed`（改走原 Sale resolve），位置、挂单、占用与买方侧事实零变化。
#[tokio::test]
async fn postgres_consignment_loss_confirm_refuses_a_funded_open_sale() {
    let world = World::sold(Closure::None).await;
    fund_open_sale(&world.owner, world.deposit.store()).await;
    let proof_id = record_proof(&world, 1).await;
    let buyer_before = buyer_side_snapshot(&world).await;
    let position = world.position().await;
    let listing = world.listing().await;

    let refused = world
        .deposit
        .confirm_position_loss(confirm(1, 3, proof_id, 0x41))
        .await
        .expect_err("买家已付款，不得按 position 办结");

    assert_eq!(rejection(refused), DepositBusinessRejection::ActionNotAllowed);
    assert_still_held(&world, position, listing).await;
    assert_eq!(buyer_side_snapshot(&world).await, buyer_before);
}

/// 反例：证据不属于这件实物（证据 ID 不存在）→ evidence_unproven；position 版本落后 → stale_version。
/// 两者都是确定拒绝，零损失行、位置与挂单原样。
#[tokio::test]
async fn postgres_consignment_loss_confirm_refuses_unproven_evidence_and_stale_versions() {
    let world = World::sold(Closure::CancelledUnpaid).await;
    let proof_id = record_proof(&world, 1).await;
    let position = world.position().await;
    let listing = world.listing().await;

    let unproven = world
        .deposit
        .confirm_position_loss(confirm(
            1,
            3,
            Uuid::from_u128(0x7c4b_0000_0000_4000_8000_0000_0000_0f00),
            0x41,
        ))
        .await
        .expect_err("证据不存在");
    assert_eq!(rejection(unproven), DepositBusinessRejection::EvidenceUnproven);
    let stale = world
        .deposit
        .confirm_position_loss(confirm(2, 2, proof_id, 0x42))
        .await
        .expect_err("版本落后");
    assert_eq!(rejection(stale), DepositBusinessRejection::StaleVersion);

    assert_still_held(&world, position, listing).await;
}
