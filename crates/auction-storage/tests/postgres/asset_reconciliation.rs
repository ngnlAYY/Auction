//! 资产对账与偿付报告在迁移后的真实 schema 上以运行账号执行。
//!
//! 源码内的 `postgres_*` 用例把报告 SQL 改写到 `pg_temp` 影子表，覆盖难以在真实约束下构造的
//! 异常态；这里只用真实表与生产入口，证明报告 SQL 与当前列、约束和 `auction_runtime` 权限一致：
//! - 一件经投稿进入 live 场次的应持装备未被 Armory 观察到，计为缺失并给出样本；
//! - 一件数据库已知的普通库存与一件未知个人装备不构成差异；
//! - 余额证据从未刷新时偿付结论为 `unknown`，不声称充足；
//! - 两份报告都追加到 `audit_records`，并能经最近报告投影原样读回。
#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::reconciliation::{
    AssetArmoryObservation, AssetReconciliationStore, CustodyMissingSample,
};
use auction_domain::SettlementWorld;
use auction_time::{Duration, Timestamp};

use crate::support;
use crate::support::fixtures::{policy_snapshot, stable_id};

const SEED: &str = "asset-reconciliation";
/// 投稿来源引用的外部装备 ID：应持但未被观察到。
const MISSING_EQUIPMENT: &str = "9300000";
/// 数据库已知、但不在任何应持集合内的普通库存。
const STOCK_EQUIPMENT: &str = "9300001";
/// 数据库未知的观察项，按个人装备计数。
const PERSONAL_EQUIPMENT: &str = "9300002";

/// 写入一场 main 世界的 live 场次、一条 Admin 投稿与一件普通库存；全部以运行账号写入。
async fn seed(pool: &sqlx::PgPool) {
    let now = Timestamp::now();
    let opened = now - Duration::hours(1);
    let deadline = now + Duration::hours(2);
    let auction_id = stable_id(SEED, "auction");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, 'main', 1, 'asset reconciliation', 'live', $2, $2, $3, $3, $2, $2, $4)",
    )
    .bind(auction_id)
    .bind(opened)
    .bind(deadline)
    .bind(policy_snapshot(deadline, 50_000, 30_000))
    .execute(pool)
    .await
    .expect("写入测试场次");

    let command_id = stable_id(SEED, "submission-command");
    let submission_id = stable_id(SEED, "submission");
    let mut fingerprint = [0_u8; 32];
    fingerprint[..16].copy_from_slice(submission_id.as_bytes());
    fingerprint[16..].copy_from_slice(command_id.as_bytes());
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'admin_intake_item', $2, $3, 'accepted')",
    )
    .bind(command_id)
    .bind(auction_id)
    .bind(fingerprint.as_slice())
    .execute(pool)
    .await
    .expect("写入投稿来源命令");
    sqlx::query(
        "INSERT INTO public.auction_submissions
            (id, source_command_id, auction_id, seller, received_at,
             source_kind, source_reference, source_fingerprint)
         VALUES ($1, $2, $3, 'reconcile-seller', $4, 'admin', $5, $6)",
    )
    .bind(submission_id)
    .bind(command_id)
    .bind(auction_id)
    .bind(opened)
    .bind(format!("hentaiverse-equipment-v1:{MISSING_EQUIPMENT}"))
    .bind(fingerprint.as_slice())
    .execute(pool)
    .await
    .expect("写入投稿记录");

    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, collected_at)
         VALUES ($1, 'main', $2, 'asset reconciliation stock', 'weapon', $3)",
    )
    .bind(stable_id(SEED, "stock-equipment"))
    .bind(STOCK_EQUIPMENT)
    .bind(opened)
    .execute(pool)
    .await
    .expect("写入普通库存装备");
}

fn observation(world: SettlementWorld, equipment_ids: &[&str]) -> AssetArmoryObservation {
    AssetArmoryObservation {
        world,
        observed_at: Timestamp::now(),
        equipment_ids: equipment_ids.iter().map(|id| (*id).to_owned()).collect(),
        complete: true,
        failure_code: None,
    }
}

#[tokio::test]
async fn postgres_asset_reports_run_on_migrated_schema_as_runtime_account() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    seed(store.postgres_pool()).await;

    // 两个世界都给出完整观察，计数与样本查询才会真正执行。
    let custody = store
        .reconcile_custody(vec![
            observation(SettlementWorld::Isekai, &[]),
            observation(SettlementWorld::Main, &[STOCK_EQUIPMENT, PERSONAL_EQUIPMENT]),
        ])
        .await
        .expect("生成装备持有报告");
    assert!(custody.complete, "两个世界的完整观察都必须进入报告");
    assert_eq!(custody.failure_code, None, "报告 SQL 必须在真实 schema 上执行成功");
    assert!(!custody.consistent, "未观察到的应持装备使报告不一致");
    assert!(!custody.asset_actions_authorized);
    let main = custody.worlds.iter().find(|world| world.world == "main").expect("main 报告");
    assert_eq!(main.observation, "observed");
    assert_eq!(
        (
            main.expected_count,
            main.observed_count,
            main.missing_count,
            main.stock_count,
            main.personal_count,
            main.unexpected_count,
        ),
        (1, 2, 1, 1, 1, 0)
    );
    assert_eq!(
        main.missing_samples,
        [CustodyMissingSample {
            external_equipment_id: MISSING_EQUIPMENT.to_owned(),
            reason: "submission_live".to_owned(),
        }]
    );
    assert!(!main.samples_truncated);
    let isekai = custody.worlds.iter().find(|world| world.world == "isekai").expect("isekai 报告");
    assert_eq!(isekai.observation, "observed");
    assert_eq!((isekai.expected_count, isekai.missing_count), (0, 0));

    let solvency = store.report_solvency(3_600).await.expect("生成偿付报告");
    assert!(solvency.complete, "偿付查询必须在真实 schema 上执行成功");
    assert_eq!(solvency.failure_code, None);
    assert!(!solvency.solvent, "没有余额证据时不得声称偿付充足");
    assert_eq!(solvency.worlds.len(), 2);
    // 迁移只建立从未刷新的健康行（余额 0、证据 unknown），不能被当作可判定的余额。
    for world in &solvency.worlds {
        assert_eq!(world.required_credits, 0, "{} 没有待付负债", world.world);
        assert_eq!(world.balance_observed_at, None);
        assert_eq!(world.evidence_status, "unknown");
        assert_eq!(world.verdict, "unknown", "{} 缺少余额证据", world.world);
    }

    // 两份报告都已追加为审计事实，最近报告投影读回的内容与返回值一致。
    let latest = store.latest_asset_reports().await.expect("读取最近报告");
    assert_eq!(latest.custody, Some(custody));
    assert_eq!(latest.solvency, Some(solvency));

    store.postgres_pool().close().await;
}
