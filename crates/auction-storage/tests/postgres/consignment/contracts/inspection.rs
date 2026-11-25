//! 巡检的前置身份比较对寄售行失败关闭：NULL 安全并包含合同标识。

use auction_application::consignment::ConsignmentGraphStore;
use auction_domain::SettlementWorld;

use super::super::deposit::support::succeeds;
use super::fixture::seed_main_custody;
use super::graph_write::{sale_graph, seed_funding};
use crate::support::TestDatabase;

const AUCTION: &str = "c5e00000-0000-4000-8000-000000000001";
const LOT: &str = "c5e00000-0000-4000-8000-000000000002";
const EQUIPMENT: &str = "c5e00000-0000-4000-8000-000000000003";
const COMMAND: &str = "c5e00000-0000-4000-8000-000000000004";
const AUCTION_OBLIGATION: &str = "c5e00000-0000-4000-8000-000000000005";

#[tokio::test]
async fn postgres_consignment_prerequisite_pointing_to_auction_obligation_is_reported() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    seed_funding(pool).await;
    runtime.write_graph(sale_graph(SettlementWorld::Main, 1000)).await.expect("写入合同图");
    let database_name: String =
        sqlx::query_scalar("SELECT current_database()").fetch_one(pool).await.expect("数据库名");

    let violations = |report: auction_storage::ConsistencyInspectionReport| -> Vec<String> {
        report
            .checks
            .iter()
            .flat_map(|check| check.samples.iter().map(|sample| sample.reason.clone()))
            .filter(|reason| reason == "obligation_prerequisite_identity_mismatch")
            .collect()
    };
    let clean = runtime.inspect_consistency(&database_name).await.expect("巡检");
    assert!(violations(clean).is_empty(), "合法寄售图的前置同合同，不得报告");

    // 一条拍卖义务（四个身份列非空、合同为空）；寄售卖款的前置改指向它。
    let now = auction_time::Timestamp::now();
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, policy_snapshot)
         VALUES ($1::uuid, 'main', 1, 'prerequisite identity', 'live', $2, $2, $2, $2, $3)",
    )
    .bind(AUCTION)
    .bind(now)
    .bind(crate::support::fixtures::policy_snapshot(now, 50_000, 30_000))
    .execute(pool)
    .await
    .expect("写入场次");
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.auction_commands
                (command_id, command_kind, aggregate_id, request_hash, result_code)
             VALUES ('{COMMAND}', 'settlement-test', '{AUCTION}',
                     decode(repeat('01', 32), 'hex'), 'accepted');
             INSERT INTO public.equipment
                (id, world_id, external_equipment_id, name, category, attributes, collected_at)
             VALUES ('{EQUIPMENT}', 'main', '901', 'Auction gear', 'Weapon', '{{}}'::jsonb,
                     '2026-01-01T00:00:00Z');
             INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price,
                 category_key, category_number)
             VALUES ('{LOT}', '{AUCTION}', '{EQUIPMENT}', 1, 'unsold', 100, 'Weapon', 1);
             INSERT INTO public.settlement_obligations
                (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                 obligation_kind, business_state, recipient_game_identity, amount,
                 carrier_credit, equipment_id)
             VALUES ('{AUCTION_OBLIGATION}', '{AUCTION}', '{LOT}', 'main', '{COMMAND}', 1,
                     'unsold_return', 'ready', 'FakeSeller', 0, 0, '{EQUIPMENT}');
             INSERT INTO public.settlement_obligation_fulfillments
                (obligation_id, execution_state, dispatch_world, delivery_operation)
             VALUES ('{AUCTION_OBLIGATION}', 'pending', 'main', 'send');
             UPDATE public.settlement_obligations
             SET prerequisite_obligation_id = '{AUCTION_OBLIGATION}'
             WHERE contract_id IS NOT NULL AND obligation_kind = 'seller_payout';"
        ),
    )
    .await;

    let report = runtime.inspect_consistency(&database_name).await.expect("巡检");
    assert_eq!(violations(report).len(), 1, "寄售义务的前置指向拍卖义务必须报违规");
}
