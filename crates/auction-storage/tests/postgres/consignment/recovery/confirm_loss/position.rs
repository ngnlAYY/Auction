//! 7C-4b 灭失确认（头号验收）：当前指针仍在已签 Sale 的代 g，Sale 已封闭、原件已收回，Admin 直接
//! `confirm_position_loss`——position `lost`、挂单 `closed/loss_confirmed`、一个案件一个回执，指针始终留在 g，
//! 零退款零新代，旧 Sale、旧回执与旧买家资金逐项不变；重放不增案。另验从未成交的件同样按 position 办结。
//!
//! 保护的不变量：1（同命令重放原回执、同 ID 异请求冲突）、2（指针不动）、3（事实、迁移、释放、建案、
//! 挂单关闭与回执同提）、6（终局只有一次）。

use auction_application::consignment::{
    ConfirmPositionLossCommand, ConsignmentError, CustodyRelistStore, DepositBusinessRejection,
    PositionLossReceipt, PositionLossStore, PurchaseCommand, PurchaseOutcome, PurchaseStore,
    RelistCommand,
};
use auction_application::core::RequestHash;
use auction_domain::UserId;
use auction_domain::consignment::purchase::PurchaseInput;
use auction_domain::consignment::{ListingPrice, ListingState, ListingVersion};
use uuid::Uuid;

use super::super::super::deposit::support::LISTING;
use super::super::sale_recovery::{BUYER_3, Closure, PRICE, World, command_id, id, seller};
use super::{
    all_command_ids, buyer_side_snapshot, claim_released, confirm, loss_rows, prior_receipts,
    record_proof, rejection,
};

/// 头号验收：见模块文档。
#[tokio::test]
async fn postgres_consignment_loss_confirm_after_sealed_sale_keeps_pointer_and_buyer_facts() {
    let world = World::sold(Closure::CancelledUnpaid).await;
    let proof_id = record_proof(&world, 1).await;
    assert_eq!(world.position().await, ("committed".to_owned(), Some(1), 3));
    let buyer_before = buyer_side_snapshot(&world).await;
    let prior_ids = all_command_ids(&world).await;
    let prior_before = prior_receipts(&world, &prior_ids).await;
    let available_before = world.listing_events("available").await;

    let command = confirm(1, 3, proof_id, 0x41);
    let receipt: PositionLossReceipt =
        world.deposit.confirm_position_loss(command.clone()).await.expect("确认灭失");

    // 指针始终为 g，position lost，挂单 closed/loss_confirmed。
    assert_eq!(world.position().await, ("lost".to_owned(), Some(1), 4));
    assert_eq!(world.listing().await, ("closed".to_owned(), 7));
    assert_eq!((receipt.state, receipt.listing_version.value()), (ListingState::Closed, 7));
    assert_eq!(receipt.position_version, 4);
    let reason: Option<String> =
        sqlx::query_scalar("SELECT reason_code FROM public.consignment_listings WHERE id = $1")
            .bind(id(LISTING))
            .fetch_one(&world.pool)
            .await
            .expect("读取关闭原因");
    assert_eq!(reason.as_deref(), Some("loss_confirmed"));
    assert!(claim_released(&world).await);
    // 恰一个事实、一个案件、一个开案记录、一个卖家回执、一条原身份通知；回执引用的就是这些行。
    assert_eq!(loss_rows(&world).await, [1, 1, 1, 1, 1]);
    let (case_id, receipt_id): (Uuid, Uuid) =
        sqlx::query_as("SELECT case_id, receipt_id FROM public.consignment_loss_receipts")
            .fetch_one(&world.pool)
            .await
            .expect("读取案件回执");
    assert_eq!((receipt.loss_case_id, receipt.seller_receipt_id), (case_id, receipt_id));
    // 零退款、零新代、旧 Sale / 旧回执 / 旧买家资金逐项不变；没有产生 available 事件。
    assert_eq!(buyer_side_snapshot(&world).await, buyer_before);
    assert_eq!(world.generations().await.len(), 1);
    assert_eq!(prior_receipts(&world, &prior_ids).await, prior_before);
    assert_eq!(world.listing_events("available").await, available_before);
    assert_eq!(world.listing_events("loss_confirmed").await, 1);

    // 私有 reason 只进审计 metadata（`loss_reason`），不在账本回执里。
    let audited: Option<String> = sqlx::query_scalar(
        "SELECT metadata->>'loss_reason' FROM public.audit_records
         WHERE entity_id = $1 AND action = 'consignment.confirm_position_loss'",
    )
    .bind(command.command_id.as_uuid().to_string())
    .fetch_one(&world.pool)
    .await
    .expect("读取审计");
    assert_eq!(audited.as_deref(), Some("fake loss reason"));
    let in_receipt: bool = sqlx::query_scalar(
        "SELECT result_body::text LIKE '%fake loss reason%' FROM public.consignment_commands
         WHERE command_id = $1",
    )
    .bind(command.command_id.as_uuid())
    .fetch_one(&world.pool)
    .await
    .expect("读取回执");
    assert!(!in_receipt);

    // 重放：同命令同请求返回原回执且不增案；同 ID 异请求为 command_id_conflict。
    let replay = world.deposit.confirm_position_loss(command.clone()).await.expect("重放");
    assert_eq!(replay, receipt);
    assert_eq!(loss_rows(&world).await, [1, 1, 1, 1, 1]);
    let conflict = world
        .deposit
        .confirm_position_loss(ConfirmPositionLossCommand {
            request_hash: RequestHash::new([0x42; 32]),
            ..command
        })
        .await
        .expect_err("同 ID 异请求");
    assert_eq!(conflict, ConsignmentError::CommandIdConflict);

    // 另一条新命令对已终局的件再确认：确定拒绝，不增案、不改任何事实。
    let again = world
        .deposit
        .confirm_position_loss(confirm(2, 4, proof_id, 0x43))
        .await
        .expect_err("已终局不得再确认");
    assert_eq!(rejection(again), DepositBusinessRejection::ActionNotAllowed);
    assert_eq!(loss_rows(&world).await, [1, 1, 1, 1, 1]);
    assert_eq!(world.position().await, ("lost".to_owned(), Some(1), 4));
}

/// 正例：从未成交的件（当前代没有合同，挂单仍可售）同样按 position 办结，占用释放，不产生任何买方事实。
#[tokio::test]
async fn postgres_consignment_loss_confirm_closes_a_never_sold_position() {
    let world = World::taken().await;
    let proof_id = record_proof(&world, 1).await;
    let before = buyer_side_snapshot(&world).await;
    assert_eq!(world.listing().await, ("available".to_owned(), 5));

    let receipt =
        world.deposit.confirm_position_loss(confirm(1, 2, proof_id, 0x41)).await.expect("确认灭失");

    assert_eq!(world.position().await.0, "lost");
    assert_eq!(
        world.listing().await,
        ("closed".to_owned(), receipt.listing_version.value().cast_signed())
    );
    assert!(claim_released(&world).await);
    assert_eq!(loss_rows(&world).await, [1, 1, 1, 1, 1]);
    assert_eq!(buyer_side_snapshot(&world).await, before);
}

/// 卖家把止售件重列为 g+1（挂单版本 6 → 7）。
async fn relist_unsold(world: &World) {
    let receipt = world
        .deposit
        .relist(RelistCommand {
            command_id: command_id(0x4b1),
            seller_user_id: seller(),
            listing_id: id(LISTING),
            expected_version: ListingVersion::new(6).expect("版本"),
            request_hash: RequestHash::new([0x61; 32]),
            request_id: None,
        })
        .await
        .expect("重列 g+1");
    assert_eq!((receipt.state, receipt.generation), (ListingState::Available, 2));
}

/// D9 §11.1：g 的旧买家未付、Sale 已取消、原件在库，重列 g+1 后未售即确认灭失——按 position 关 g+1，
/// 指针留在 g+1，恰一个案件与卖家回执；旧 Sale、旧回执、旧买家资金逐项不变，不新增成交、退款或处分代；
/// 重放不新建。
///
/// 保护的不变量：1（重放原回执）、2（锁内按当前代判定）、3（终局事实同提）、6（终局只有一次）。
#[tokio::test]
async fn postgres_consignment_loss_confirm_closes_an_unsold_relist_generation() {
    let world = World::sold(Closure::CancelledUnpaid).await;
    relist_unsold(&world).await;
    assert_eq!(world.position().await, ("in_custody".to_owned(), Some(2), 5));
    let proof_id = record_proof(&world, 1).await;
    let buyer_before = buyer_side_snapshot(&world).await;
    let prior_ids = all_command_ids(&world).await;
    let prior_before = prior_receipts(&world, &prior_ids).await;

    let command = confirm(1, 5, proof_id, 0x41);
    let receipt = world.deposit.confirm_position_loss(command.clone()).await.expect("确认灭失");

    assert_eq!(world.position().await, ("lost".to_owned(), Some(2), 6));
    assert_eq!(world.listing().await.0, "closed");
    assert_eq!(receipt.state, ListingState::Closed);
    assert!(claim_released(&world).await);
    assert_eq!(loss_rows(&world).await, [1, 1, 1, 1, 1]);
    // 买方侧快照覆盖合同、Sale、封闭、收款、退款、费用、派发、义务、投递与处分代：逐项不变。
    assert_eq!(buyer_side_snapshot(&world).await, buyer_before);
    assert_eq!(prior_receipts(&world, &prior_ids).await, prior_before);

    let replay = world.deposit.confirm_position_loss(command).await.expect("重放");
    assert_eq!(replay, receipt);
    assert_eq!(loss_rows(&world).await, [1, 1, 1, 1, 1]);
}

/// 反例：g+1 已被第二位买家购买（当前代有开放 Sale）。灭失确认按当前代判定，确定拒绝，零损失行，
/// position 与挂单原样。
#[tokio::test]
async fn postgres_consignment_loss_confirm_refuses_a_sold_relist_generation() {
    let world = World::sold(Closure::CancelledUnpaid).await;
    relist_unsold(&world).await;
    let bought = world
        .purchases
        .purchase(PurchaseCommand {
            command_id: command_id(0x4b2),
            request_hash: RequestHash::new([0x62; 32]),
            buyer_user_id: UserId::from_uuid(id(BUYER_3)),
            input: PurchaseInput {
                listing_id: id(LISTING),
                expected_version: ListingVersion::new(7).expect("版本"),
                expected_price: ListingPrice::new(PRICE).expect("价格"),
            },
            request_id: None,
        })
        .await
        .expect("第二位买家购买");
    assert!(matches!(bought, PurchaseOutcome::Purchased(_)), "{bought:?}");
    let proof_id = record_proof(&world, 1).await;
    let position = world.position().await;
    let listing = world.listing().await;

    let refused = world
        .deposit
        .confirm_position_loss(confirm(1, position.2.cast_unsigned(), proof_id, 0x41))
        .await
        .expect_err("当前代有开放 Sale，不得按 position 办结");

    assert_eq!(rejection(refused), DepositBusinessRejection::ActionNotAllowed);
    assert_eq!(loss_rows(&world).await, [0, 0, 0, 0, 0]);
    assert_eq!((world.position().await, world.listing().await), (position, listing));
}
