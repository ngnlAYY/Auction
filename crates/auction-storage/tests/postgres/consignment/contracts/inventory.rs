//! 库存对账的寄售代管分支：代管中的 position 计入应持装备，季末终态与未取得的不计入；
//! 同一件寄售装备不因合同图的义务被重复计入（报表只读，不授权资产动作）。

use auction_application::consignment::ConsignmentGraphStore;
use auction_application::reconciliation::{
    AssetArmoryObservation, AssetReconciliationStore, CustodyMissingSample,
    CustodyReconciliationReport,
};
use auction_domain::SettlementWorld;
use auction_storage::DatabaseStore;
use auction_time::Timestamp;

use super::super::deposit::support::{LISTING, POSITION, PROOF_COMMAND, seed_accepted, succeeds};
use super::fixture::{WINDOW, seed_isekai_custody, seed_main_custody};
use super::graph_write::{sale_graph, seed_funding};
use crate::support::TestDatabase;

const FUTURE: &str = "transaction_timestamp() + interval '10 days'";

fn observation(world: SettlementWorld, equipment_ids: &[&str]) -> AssetArmoryObservation {
    AssetArmoryObservation {
        world,
        observed_at: Timestamp::now(),
        equipment_ids: equipment_ids.iter().map(|id| (*id).to_owned()).collect(),
        complete: true,
        failure_code: None,
    }
}

/// 两个世界的完整观察；`isekai`/`main` 分别是各自观察到的外部装备 ID。
async fn report(
    store: &DatabaseStore,
    isekai: &[&str],
    main: &[&str],
) -> CustodyReconciliationReport {
    let report = store
        .reconcile_custody(vec![
            observation(SettlementWorld::Isekai, isekai),
            observation(SettlementWorld::Main, main),
        ])
        .await
        .expect("生成装备持有报告");
    assert!(report.complete && report.failure_code.is_none(), "报告 SQL 必须执行成功");
    assert!(!report.asset_actions_authorized, "报表不授权任何资产动作");
    report
}

/// (应持, 缺失, 缺失样本) 按世界读取。
fn world(
    report: &CustodyReconciliationReport,
    name: &str,
) -> (i64, i64, Vec<CustodyMissingSample>) {
    let world = report.worlds.iter().find(|world| world.world == name).expect("世界报告");
    (world.expected_count, world.missing_count, world.missing_samples.clone())
}

fn sample(reason: &str) -> Vec<CustodyMissingSample> {
    vec![CustodyMissingSample { external_equipment_id: "42".to_owned(), reason: reason.to_owned() }]
}

/// 季窗已结束并写下季末终态：position 终结、事件追加、占用同事务释放。
fn season_end_sql() -> String {
    format!(
        "ALTER TABLE public.consignment_season_windows DISABLE TRIGGER USER;
         UPDATE public.consignment_season_windows
         SET end_utc = transaction_timestamp() - interval '1 day',
             admission_cutoff_utc = transaction_timestamp() - interval '2 days'
         WHERE id = '{WINDOW}';
         UPDATE public.consignment_custody_positions
         SET state = 'season_ended', version = 3, updated_at = transaction_timestamp()
         WHERE id = '{POSITION}';
         INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', 3, '{PROOF_COMMAND}', 'season_ended', 'in_custody',
                 'season_ended', NULL, '{LISTING}', 4, transaction_timestamp());
         UPDATE public.consignment_equipment_claims
         SET reservation_id = NULL, position_id = NULL, revision = revision + 1,
             changed_at = transaction_timestamp(), last_fact_id = '{POSITION}'
         WHERE world_id = 'isekai' AND canonical_eid = '42';"
    )
}

#[tokio::test]
async fn postgres_consignment_in_custody_position_is_expected_equipment_until_season_end() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    seed_isekai_custody(owner.postgres_pool(), FUTURE).await;

    // 正例：代管中的件计入 isekai 应持；Armory 没有观察到时是缺失样本，原因 consignment_in_custody。
    let missing = report(&runtime, &[], &[]).await;
    assert_eq!(world(&missing, "isekai"), (1, 1, sample("consignment_in_custody")));
    // 反例：世界隔离，main 不因 isekai 的代管件计数。
    assert_eq!(world(&missing, "main"), (0, 0, vec![]));
    // 正例：Armory 观察到该件，应持仍是 1，但不再缺失。
    let observed = report(&runtime, &["42"], &[]).await;
    assert_eq!(world(&observed, "isekai"), (1, 0, vec![]));

    // 季末终态后不再计入应持（决定 21：季末清空不赔）。
    succeeds(owner.postgres_pool(), &season_end_sql()).await;
    let ended = report(&runtime, &[], &[]).await;
    assert_eq!(world(&ended, "isekai"), (0, 0, vec![]));
}

#[tokio::test]
async fn postgres_consignment_untaken_position_is_not_expected_equipment() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    // 已接受但尚未取得（awaiting_take）：实物还不在 Bot 手里。
    seed_accepted(owner.postgres_pool()).await;

    let untaken = report(&runtime, &[], &[]).await;
    assert_eq!(world(&untaken, "main"), (0, 0, vec![]));
}

#[tokio::test]
async fn postgres_consignment_contract_graph_does_not_double_count_custody() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    seed_main_custody(owner.postgres_pool()).await;
    seed_funding(owner.postgres_pool()).await;

    // 代管中、尚无合同图：只由寄售分支计入一次。
    let before = report(&runtime, &[], &[]).await;
    assert_eq!(world(&before, "main"), (1, 1, sample("consignment_in_custody")));

    // 写入 Sale 图后义务带 custody_equipment_id、equipment_id 为空，义务分支不会再计一次，
    // 也不会因寄售义务（付款腿 ready）改变应持与原因。
    runtime.write_graph(sale_graph(SettlementWorld::Main, 1000)).await.expect("写入合同图");
    let after = report(&runtime, &[], &[]).await;
    assert_eq!(world(&after, "main"), (1, 1, sample("consignment_in_custody")));
}
