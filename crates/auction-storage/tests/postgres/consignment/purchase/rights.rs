//! 购买事务登记权利（7D-6）：买方 / 卖方各一项权利，ID 等于领域与 SQL 的同一派生，重放不增行。

use auction_application::consignment::{PurchaseOutcome, PurchaseStore};
use auction_domain::consignment::rights::{RightRole, right_id};
use uuid::Uuid;

use super::fixture::{BUYER, Fixture, purchase};

type RightRow = (Uuid, String, Uuid, String, Vec<String>);

async fn rights(fixture: &Fixture) -> Vec<RightRow> {
    sqlx::query_as(
        "SELECT r.right_id, r.kind, r.beneficiary_user_id, r.frozen_identity, s.allowed_actions
         FROM public.consignment_rights AS r
         JOIN public.consignment_right_states AS s ON s.right_id = r.right_id AND s.version = 1
         ORDER BY r.kind",
    )
    .fetch_all(fixture.pool())
    .await
    .expect("读取权利")
}

/// 购买同事务登记买方 `continue_order` 与卖方 `claim_payment`：每合同每角色一行，ID 与 SQL
/// `c7_right_id` 逐字节一致；买方付款腿已 ready 所以有动作，卖款腿仍 blocked 所以卖方暂无动作；
/// 同命令重放不增权利行与状态版本。
#[tokio::test]
async fn postgres_consignment_rights_purchase_registers_one_right_per_contract_and_role() {
    let fixture = Fixture::available().await;
    let outcome = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("购买");
    let PurchaseOutcome::Purchased(receipt) = outcome else {
        panic!("应成交：{outcome:?}");
    };
    let contract = receipt.contract_id.as_uuid();
    let (buyer_user, seller_user): (Uuid, Uuid) = sqlx::query_as(
        "SELECT sale.buyer_user_id, contract.seller_user_id
         FROM public.consignment_contracts AS contract
         JOIN public.consignment_sales AS sale USING (contract_id)",
    )
    .fetch_one(fixture.pool())
    .await
    .expect("合同双方");
    let first = rights(&fixture).await;
    assert_eq!(first.len(), 2);
    let buyer = right_id(contract, RightRole::Buyer);
    let seller = right_id(contract, RightRole::Seller);
    assert_ne!(buyer, seller);
    // 按 kind 排序：claim_payment（卖方）在前，continue_order（买方）在后。
    assert_eq!(
        (first[0].0, first[0].1.as_str(), first[0].2),
        (seller, "claim_payment", seller_user)
    );
    assert!(first[0].4.is_empty());
    assert_eq!(
        (first[1].0, first[1].1.as_str(), first[1].2),
        (buyer, "continue_order", buyer_user)
    );
    assert_eq!(first[1].4, vec!["continue_order".to_owned()]);
    for (role, expected) in [("buyer", buyer), ("seller", seller)] {
        let derived: Uuid = sqlx::query_scalar("SELECT public.c7_right_id($1, $2)")
            .bind(contract)
            .bind(role)
            .fetch_one(fixture.pool())
            .await
            .expect("SQL 派生");
        assert_eq!(derived, expected, "领域派生必须与 SQL c7_right_id 逐字节一致");
    }
    // 反例：同命令重放不增行。
    fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("重放");
    assert_eq!(rights(&fixture).await, first);
    assert_eq!(fixture.count("consignment_right_states").await, 2);
}
