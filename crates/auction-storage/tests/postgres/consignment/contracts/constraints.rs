//! 来源恰一、Sale 金额守恒、Withdrawal 不带钱与来源列不可改写（不变量 6、8）。

use super::super::deposit::support::{
    ASSET, DECIDE, REGISTER, USER_2, check, execute, fails, succeeds,
};
use super::fixture::{CONTRACT, Graph, OBLIGATION_1, seed_main_custody};
use crate::support::TestDatabase;

/// 一条形状合法的 buyer_refund 义务；调用方只改世界与合同列以构造单一违规。
fn refund(world: &str, contract_columns: &str) -> String {
    format!(
        "INSERT INTO public.settlement_obligations
            (id, world_id, policy_version, obligation_kind, business_state, recipient_user_id,
             amount, carrier_credit, prerequisite_obligation_id, contract_id, contract_kind,
             custody_equipment_id)
         VALUES ('c5100000-0000-4000-8000-0000000000f1', '{world}', 1, 'buyer_refund',
                 'contingent', '{USER_2}', 1000, 0, '{OBLIGATION_1}', {contract_columns});"
    )
}

#[tokio::test]
async fn postgres_consignment_contract_obligation_has_exactly_one_origin() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;

    // 无来源与双来源都被来源 CHECK 拒绝。
    let neither = "INSERT INTO public.settlement_obligations
            (id, world_id, policy_version, obligation_kind, business_state,
             recipient_game_identity, amount, carrier_credit)
         VALUES ('c5100000-0000-4000-8000-0000000000f2', 'main', 1, 'unsold_return', 'ready',
                 'FakeSellerOne', 0, 0);";
    check(pool, neither, "c5_obligation_origin_ck").await;
    let both = format!(
        "INSERT INTO public.settlement_obligations
            (id, auction_id, lot_id, origin_command_id, equipment_id, world_id, policy_version,
             obligation_kind, business_state, recipient_game_identity, amount, carrier_credit,
             contract_id, contract_kind, custody_equipment_id)
         VALUES ('c5100000-0000-4000-8000-0000000000f3', '{REGISTER}', '{REGISTER}',
                 '{REGISTER}', '{ASSET}', 'main', 1, 'withdrawn_return', 'ready',
                 'FakeSellerOne', 0, 0, '{CONTRACT}', 'withdrawal', '{ASSET}');"
    );
    check(pool, &both, "c5_obligation_origin_ck").await;
    // 寄售来源不能带投稿。
    let with_submission = format!(
        "INSERT INTO public.settlement_obligations
            (id, world_id, policy_version, obligation_kind, business_state,
             recipient_game_identity, amount, carrier_credit, submission_id, contract_id,
             contract_kind, custody_equipment_id)
         VALUES ('c5100000-0000-4000-8000-0000000000f4', 'main', 1, 'withdrawn_return', 'ready',
                 'FakeSellerOne', 0, 0, '{REGISTER}', '{CONTRACT}', 'withdrawal', '{ASSET}');"
    );
    check(pool, &with_submission, "c5_obligation_origin_ck").await;

    // 合同头先行，坏义务在语句末被外键或 CHECK 拒绝。
    let head = Graph::sale_main().head_sql(DECIDE);
    let wrong_equipment = refund("main", &format!("'{CONTRACT}', 'sale', '{REGISTER}'"));
    fails(pool, &(head.clone() + &wrong_equipment), "23503", Some("c5_obligation_equipment_fk"))
        .await;
    let wrong_world = refund("isekai", &format!("'{CONTRACT}', 'sale', '{ASSET}'"));
    fails(pool, &(head.clone() + &wrong_world), "23503", Some("c5_obligation_world_fk")).await;
    let wrong_kind = format!(
        "INSERT INTO public.settlement_obligations
            (id, world_id, policy_version, obligation_kind, business_state,
             recipient_game_identity, amount, carrier_credit, contract_id, contract_kind,
             custody_equipment_id)
         VALUES ('c5100000-0000-4000-8000-0000000000f5', 'main', 1, 'withdrawn_return', 'ready',
                 'FakeSellerOne', 0, 0, '{CONTRACT}', 'sale', '{ASSET}');"
    );
    fails(pool, &(head + &wrong_kind), "23514", Some("c5_obligation_kind_matrix_ck")).await;

    // 正例：合法的寄售图提交后，拍卖四列全空、合同三列全有。
    succeeds(pool, &Graph::sale_main().sql(DECIDE)).await;
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE auction_id IS NULL AND lot_id IS NULL
                                   AND origin_command_id IS NULL AND equipment_id IS NULL
                                   AND submission_id IS NULL AND contract_id IS NOT NULL
                                   AND custody_equipment_id IS NOT NULL),
                count(*)
         FROM public.settlement_obligations",
    )
    .fetch_one(pool)
    .await
    .expect("读取义务来源列");
    assert_eq!(counts, (3, 3));
}

#[tokio::test]
async fn postgres_consignment_sale_amounts_conserve_fee_and_seller_amount() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;

    // 反例：S 被篡改、F 偏离 floor 都被金额 CHECK 拒绝。
    let tampered = Graph { seller_amount: 951, ..Graph::sale_main() };
    fails(pool, &tampered.sql(DECIDE), "23514", Some("c5_sale_quote_ck")).await;
    let rounded_up = Graph { price: 999, fee: 50, seller_amount: 949, ..Graph::sale_main() };
    fails(pool, &rounded_up.sql(DECIDE), "23514", Some("c5_sale_quote_ck")).await;

    // 正例：P=999、bps=500 时 F=floor(49.95)=49，S=950。
    let floored =
        Graph { price: 999, fee: 49, seller_amount: 950, payout_amount: 950, ..Graph::sale_main() };
    succeeds(pool, &floored.sql(DECIDE)).await;
}

#[tokio::test]
async fn postgres_consignment_withdrawal_cannot_carry_money_or_a_sale() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;

    // 反例：撤回义务带金额、撤回合同挂 Sale 子项都被拒绝。
    let paid =
        Graph::withdrawal().sql(DECIDE).replace("'FakeSellerOne', 0, 0", "'FakeSellerOne', 7, 0");
    fails(pool, &paid, "23514", Some("settlement_obligations_kind_shape_check")).await;
    let sale_child = Graph::sale_main().head_sql(DECIDE).replace("'sale'", "'withdrawal'");
    fails(pool, &sale_child, "23503", Some("c5_sale_contract_fk")).await;

    // 正例：Withdrawal 只有一条零金额义务。
    succeeds(pool, &Graph::withdrawal().sql(DECIDE)).await;
    let obligations: (i64, i64) = sqlx::query_as(
        "SELECT count(*), COALESCE(sum(amount), 0)::bigint
         FROM public.settlement_obligations WHERE contract_id = $1::uuid",
    )
    .bind(CONTRACT)
    .fetch_one(pool)
    .await
    .expect("读取撤回义务");
    assert_eq!(obligations, (1, 0));
}

#[tokio::test]
async fn postgres_consignment_obligation_origin_columns_are_immutable() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    succeeds(pool, &Graph::sale_main().sql(DECIDE)).await;

    // 反例：来源与合同列任何一列改写都被触发器拒绝。
    for assignment in [
        "contract_id = NULL",
        "contract_kind = 'withdrawal'",
        "custody_equipment_id = '00000000-0000-4000-8000-000000000001'",
        "equipment_id = '00000000-0000-4000-8000-000000000001'",
        "submission_id = '00000000-0000-4000-8000-000000000001'",
    ] {
        fails(
            pool,
            &format!(
                "UPDATE public.settlement_obligations SET {assignment} WHERE id = '{OBLIGATION_1}';"
            ),
            "23514",
            Some("c5_obligation_origin_immutable"),
        )
        .await;
    }
    // 正例：非来源列照常推进。
    execute(
        pool,
        &format!(
            "UPDATE public.settlement_obligations
             SET state_version = 2, updated_at = transaction_timestamp()
             WHERE id = '{OBLIGATION_1}';"
        ),
    )
    .await
    .expect("非来源列应可更新");
    // 合同事实本身只追加。
    fails(
        pool,
        &format!("UPDATE public.consignment_contracts SET frozen_seller_identity = 'X' WHERE contract_id = '{CONTRACT}';"),
        "55000",
        None,
    )
    .await;
}
