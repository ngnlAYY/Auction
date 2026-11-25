//! 本人 v2 挂单的损失案件引用（J3、D9 §6.1“本人详情可追到损失案”）：确认灭失前为空，之后指向
//! 唯一案件，且只出现在本人读取里。
//!
//! 保护不变量 7（他人读不到挂单与案件）与 6（引用来自同一读快照的持久案件，不伪造）。

use auction_application::consignment::{OwnedListingReadV2, PositionLossStore};
use auction_domain::UserId;

use super::super::super::deposit::support::{LISTING, POSITION};
use super::super::sale_recovery::{BUYER_3, World, id, seller};
use super::{confirm, record_proof};

#[tokio::test]
async fn postgres_consignment_loss_case_reaches_the_owned_v2_listing_only_after_confirmation() {
    let world = World::taken().await;
    let before = world
        .deposit
        .owned_detail_v2(seller(), id(LISTING))
        .await
        .expect("读本人挂单")
        .expect("本人可见");
    assert_eq!(before.custody.position.map(|position| position.position_id), Some(id(POSITION)));
    assert_eq!(before.custody.loss_case_id, None, "确认灭失前没有案件");

    let proof_id = record_proof(&world, 1).await;
    let receipt =
        world.deposit.confirm_position_loss(confirm(1, 2, proof_id, 0x41)).await.expect("确认灭失");
    let after = world
        .deposit
        .owned_detail_v2(seller(), id(LISTING))
        .await
        .expect("读本人挂单")
        .expect("本人可见");
    assert_eq!(after.custody.loss_case_id, Some(receipt.loss_case_id));
    assert_eq!(after.custody.position.map(|position| position.position_id), Some(id(POSITION)));
    let outsider = UserId::from_uuid(id(BUYER_3));
    assert!(world.deposit.owned_detail_v2(outsider, id(LISTING)).await.expect("读").is_none());
}
