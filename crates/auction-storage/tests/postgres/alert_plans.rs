//! 告警履约水位与履约终态配对口径的执行计划基准（`--ignored`，运行方式同 `crate::perf`）。
//!
//! 合成历史复用 `crate::perf::seed` 的 1 万条成交 lot 与 `settlement_returns::plans` 的结算义务、
//! 投递、恢复收口与退件案件，再按生产写入形状补齐：每个成交 lot 一条恢复记录（结算同事务写入），
//! 从未激活的或有买方退款都有带 `dormant` 标记的初始 `pending` 履约（约 9,400 条）。每种规模在同
//! 一份数据上测两组计划：
//!
//! - 现行：生产查询原文与迁移 000110 的两个部分索引；
//! - 修复前：水位入口换回“活动履约 JOIN 义务预筛业务状态”的原文、配对口径换回“全部义务 LEFT JOIN
//!   履约”的全表入口，删去现行部分索引并另建修复前谓词（`execution_state <> 'satisfied'`）的同形
//!   索引，与修复前的查询与索引一致；测完恢复现行索引。
//!
//! 随后再追加一倍成交历史（每个 lot 的付款、交付、卖方货款均已满足，或有退款未激活，另有已完成
//! 的恢复记录），重测两组计划：现行入口读出的行数与缓冲区不随历史增长，修复前的入口随或有退款
//! 增长、配对口径随义务总数增长；两组计划的读数在两种规模下逐列相同。SQL 经
//! `auction_storage::alert_benchmark_sql` 读取生产原文；只写一次性克隆库，不接触真实数据或外部
//! 服务。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use serde_json::json;
use sqlx::{AssertSqlSafe, PgPool};

use crate::alert_entry_equivalence::{legacy_paired_sql, legacy_snapshot_sql, result_row};
use crate::perf::{PlanSummary, explain, seed, write_report};
use crate::settlement_returns::plans::seed_returns;
use crate::support;

/// 迁移 000110 的两个部分索引；改名前必须同步此处。
const OPEN_INDEX: &str = "settlement_obligation_fulfillments_open_idx";
const BLOCKED_INDEX: &str = "settlement_obligations_blocked_idx";
/// 只在修复前测量期间存在的同形索引。
const LEGACY_OPEN_INDEX: &str = "perf_fulfillments_open_legacy_idx";
/// 水位查询的实参：观察时刻、超期截止（一小时阈值）、读清单、清单上限。
const SNAPSHOT_ARGUMENTS: &str = "now(), now() - interval '1 hour', true, 200";

/// 按生产写入形状补齐：结算为每个有获胜者的 lot 同事务写入恢复记录，并为每条义务写入初始
/// `pending` 履约，或有义务带 `dormant` 标记（`settlement/obligations.rs`）。退件合成历史只为
/// 少数 lot 写了恢复记录，也只为已激活的退款写了履约。
const PRODUCTION_SHAPE_SQL: &str = r#"
INSERT INTO public.settlement_recoveries
  (id, auction_id, lot_id, world_id, equipment_id, winner_user_id, payment_obligation_id,
   equipment_obligation_id, seller_payout_obligation_id, buyer_refund_obligation_id,
   policy_version, payment_deadline_at, stage, state_version, created_at, updated_at,
   completed_at)
SELECT md5('shape-recovery-' || payment.lot_id)::uuid, payment.auction_id, payment.lot_id,
       payment.world_id, payment.equipment_id, payment.recipient_user_id, payment.id,
       handoff.id, payout.id, refund.id, 1, payment.created_at + interval '7 days', 'completed',
       3, payment.created_at, payment.created_at + interval '2 days',
       payment.created_at + interval '2 days'
FROM public.settlement_obligations AS payment
JOIN public.settlement_obligations AS payout
  ON payout.lot_id = payment.lot_id AND payout.obligation_kind = 'seller_payout'
JOIN public.settlement_obligations AS refund
  ON refund.lot_id = payment.lot_id AND refund.obligation_kind = 'buyer_refund'
LEFT JOIN public.settlement_obligations AS handoff
  ON handoff.lot_id = payment.lot_id AND handoff.obligation_kind = 'winner_equipment_handoff'
WHERE payment.obligation_kind IN ('winner_equipment_cod', 'winner_payment_carrier_cod')
  AND NOT EXISTS (
      SELECT 1 FROM public.settlement_recoveries AS existing WHERE existing.lot_id = payment.lot_id
  );

INSERT INTO public.settlement_obligation_fulfillments
  (obligation_id, execution_state, dispatch_world, delivery_operation, dormant, created_at,
   updated_at)
SELECT o.id, 'pending', 'main', 'send', true, o.created_at, o.created_at
FROM public.settlement_obligations AS o
WHERE o.business_state = 'contingent'
  AND NOT EXISTS (
      SELECT 1 FROM public.settlement_obligation_fulfillments AS f WHERE f.obligation_id = o.id
  );
"#;

/// 追加一倍已完成的成交历史：复制每个合成场次与 lot，付款、交付与卖方货款都已满足，或有退款
/// 未激活（带标记的初始 `pending` 履约）；恢复记录随后由 [`PRODUCTION_SHAPE_SQL`] 补齐。
const EXTRA_HISTORY_SQL: &str = r#"
INSERT INTO public.auctions
  (id, world_id, sequence, title, status, settled_at, scheduled_open_at, preview_scheduled_at,
   scheduled_end_at, effective_end_at, policy_snapshot)
SELECT md5('extra-auction-' || a.id)::uuid, a.world_id, a.sequence + 100000,
       'Perf extra auction ' || a.sequence, a.status, a.settled_at, a.scheduled_open_at,
       a.preview_scheduled_at, a.scheduled_end_at, a.effective_end_at, a.policy_snapshot
FROM public.auctions AS a
WHERE a.title LIKE 'Perf auction %';

INSERT INTO public.lots
  (id, auction_id, equipment_id, lot_number, status, start_price, current_price, leader_user_id,
   effective_end_at, category_key, category_number)
SELECT md5('extra-lot-' || l.id)::uuid, md5('extra-auction-' || l.auction_id)::uuid,
       l.equipment_id, l.lot_number, l.status, l.start_price, l.current_price, l.leader_user_id,
       l.effective_end_at, l.category_key, l.category_number
FROM public.lots AS l
JOIN public.auctions AS a ON a.id = l.auction_id
WHERE a.title LIKE 'Perf auction %';

WITH copied AS (
    INSERT INTO public.settlement_obligations
      (id, auction_id, lot_id, world_id, origin_command_id, policy_version, obligation_kind,
       business_state, recipient_user_id, recipient_game_identity, amount, carrier_credit,
       equipment_id, prerequisite_obligation_id, created_at, updated_at)
    SELECT md5('extra-' || o.id)::uuid, md5('extra-auction-' || o.auction_id)::uuid,
           md5('extra-lot-' || o.lot_id)::uuid, o.world_id, o.origin_command_id, 1,
           o.obligation_kind,
           CASE WHEN o.obligation_kind = 'buyer_refund' THEN 'contingent' ELSE 'ready' END,
           o.recipient_user_id, o.recipient_game_identity, o.amount, o.carrier_credit,
           o.equipment_id,
           CASE WHEN o.prerequisite_obligation_id IS NULL THEN NULL
                ELSE md5('extra-' || o.prerequisite_obligation_id)::uuid END,
           o.created_at, o.updated_at
    FROM public.settlement_obligations AS o
    JOIN public.auctions AS a ON a.id = o.auction_id
    WHERE a.title LIKE 'Perf auction %'
    RETURNING id, obligation_kind, business_state, world_id, created_at
)
INSERT INTO public.settlement_obligation_fulfillments
  (obligation_id, execution_state, dispatch_world, delivery_operation, dormant, completed_at,
   created_at, updated_at)
SELECT copied.id,
       CASE WHEN copied.business_state = 'contingent' THEN 'pending' ELSE 'satisfied' END,
       CASE WHEN copied.obligation_kind IN (
                'seller_payout', 'buyer_refund', 'winner_payment_carrier_cod'
            ) THEN 'main'
            ELSE copied.world_id END,
       'send', copied.business_state = 'contingent',
       CASE WHEN copied.business_state = 'contingent' THEN NULL
            ELSE copied.created_at + interval '2 days' END,
       copied.created_at, copied.created_at + interval '2 days'
FROM copied;
"#;

/// 一种规模下两组计划。
struct Measured {
    snapshot: PlanSummary,
    snapshot_legacy: PlanSummary,
    paired: PlanSummary,
    paired_legacy: PlanSummary,
}

async fn explain_or_panic(pool: &PgPool, sql: &str, arguments: &str, label: &str) -> PlanSummary {
    explain(pool, sql, arguments).await.unwrap_or_else(|error| panic!("{label}：{error}"))
}

/// 现行索引定义（`pg_get_indexdef`），删去后按原文恢复。
async fn index_definitions(pool: &PgPool) -> Vec<String> {
    let definitions: Vec<String> = sqlx::query_scalar(
        "SELECT pg_get_indexdef(indexrelid) FROM pg_index
         WHERE indexrelid IN ($1::regclass, $2::regclass) ORDER BY indexrelid",
    )
    .bind(format!("public.{OPEN_INDEX}"))
    .bind(format!("public.{BLOCKED_INDEX}"))
    .fetch_all(pool)
    .await
    .expect("读取现行部分索引定义");
    assert_eq!(definitions.len(), 2, "现行部分索引应都存在");
    definitions
}

async fn execute(owner: &PgPool, statement: String, label: &str) {
    sqlx::raw_sql(AssertSqlSafe(statement))
        .execute(owner)
        .await
        .unwrap_or_else(|error| panic!("{label}：{error}"));
}

async fn measure(pool: &PgPool, owner: &PgPool, label: &str) -> Measured {
    let snapshot_sql = auction_storage::alert_benchmark_sql::fulfillment_snapshot_sql();
    let paired_sql = auction_storage::alert_benchmark_sql::paired_fulfillment_counts_sql();
    let snapshot_legacy_sql = legacy_snapshot_sql(snapshot_sql);
    let paired_legacy_sql = legacy_paired_sql(paired_sql);

    let snapshot_row = result_row(pool, snapshot_sql, SNAPSHOT_ARGUMENTS).await;
    let paired_row = result_row(pool, paired_sql, "").await;
    let snapshot = explain_or_panic(pool, snapshot_sql, SNAPSHOT_ARGUMENTS, label).await;
    let paired = explain_or_panic(pool, paired_sql, "", label).await;

    // 修复前：删去现行部分索引、建立修复前同形索引，测完恢复（DDL 只能由迁移 owner 执行）。
    let definitions = index_definitions(owner).await;
    execute(
        owner,
        format!(
            "DROP INDEX public.{OPEN_INDEX}; DROP INDEX public.{BLOCKED_INDEX};
             CREATE INDEX {LEGACY_OPEN_INDEX} ON public.settlement_obligation_fulfillments
                 (obligation_id) WHERE execution_state <> 'satisfied';
             ANALYZE public.settlement_obligation_fulfillments, public.settlement_obligations;"
        ),
        "换成修复前索引",
    )
    .await;
    assert_eq!(
        result_row(pool, &snapshot_legacy_sql, SNAPSHOT_ARGUMENTS).await,
        snapshot_row,
        "{label}：水位读数应与修复前逐列相同"
    );
    assert_eq!(
        result_row(pool, &paired_legacy_sql, "").await,
        paired_row,
        "{label}：配对口径读数应与修复前逐列相同"
    );
    let snapshot_legacy =
        explain_or_panic(pool, &snapshot_legacy_sql, SNAPSHOT_ARGUMENTS, label).await;
    let paired_legacy = explain_or_panic(pool, &paired_legacy_sql, "", label).await;
    execute(
        owner,
        format!(
            "DROP INDEX public.{LEGACY_OPEN_INDEX}; {};
             ANALYZE public.settlement_obligation_fulfillments, public.settlement_obligations;",
            definitions.join("; ")
        ),
        "恢复现行索引",
    )
    .await;
    Measured { snapshot, snapshot_legacy, paired, paired_legacy }
}

fn buffers(summary: &PlanSummary) -> i64 {
    summary.shared_hit_blocks + summary.shared_read_blocks
}

fn print_pair(label: &str, before: &PlanSummary, after: &PlanSummary) {
    println!(
        "{label}: 修复前 {:.3}ms / {} buffers -> 现行 {:.3}ms / {} buffers",
        before.execution_ms,
        buffers(before),
        after.execution_ms,
        buffers(after),
    );
    println!("  修复前节点 {:?}", before.node_types());
    println!("  现行节点 {:?}", after.node_types());
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(AssertSqlSafe(sql.to_owned())).fetch_one(pool).await.expect("统计合成数据")
}

/// 计划里是否顺序扫描了某张表。
fn seq_scans(summary: &PlanSummary, table: &str) -> bool {
    summary.node_types().iter().any(|node| node == &format!("Seq Scan[{table}]"))
}

#[tokio::test]
#[ignore = "热点基准：显式运行，不在默认 test-db 门禁中执行"]
async fn perf_alert_fulfillment_entry_and_paired_terminal_plans() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let pool = store.postgres_pool().clone();
    let maintenance = database.maintenance_store().await.expect("维护账号连接");
    let owner_pool = maintenance.postgres_pool().clone();
    seed(&pool, &owner_pool).await.expect("写入合成成交历史");
    seed_returns(&owner_pool).await.expect("写入合成退件历史");
    execute(&owner_pool, PRODUCTION_SHAPE_SQL.to_owned(), "补齐生产写入形状").await;
    execute(&owner_pool, "ANALYZE".to_owned(), "统计合成历史").await;

    let dormant_sql =
        "SELECT count(*) FROM public.settlement_obligation_fulfillments WHERE dormant";
    let failed_sql = "SELECT count(*) FROM public.settlement_obligation_fulfillments
                      WHERE execution_state = 'failed'";
    let obligations_sql = "SELECT count(*) FROM public.settlement_obligations";
    let recoveries_sql = "SELECT count(*) FROM public.settlement_recoveries";
    let scale = [
        count(&pool, obligations_sql).await,
        count(&pool, dormant_sql).await,
        count(&pool, recoveries_sql).await,
    ];
    let failed = count(&pool, failed_sql).await;
    let base = measure(&pool, &owner_pool, "基础规模").await;

    execute(&owner_pool, EXTRA_HISTORY_SQL.to_owned(), "追加一倍成交历史").await;
    execute(&owner_pool, PRODUCTION_SHAPE_SQL.to_owned(), "补齐追加历史的恢复记录").await;
    execute(&owner_pool, "ANALYZE".to_owned(), "统计追加历史").await;
    let doubled_scale = [
        count(&pool, obligations_sql).await,
        count(&pool, dormant_sql).await,
        count(&pool, recoveries_sql).await,
    ];
    assert_eq!(count(&pool, failed_sql).await, failed, "追加历史不应新增失败履约");
    let doubled = measure(&pool, &owner_pool, "双倍规模").await;

    println!(
        "合成规模（义务、或有未激活退款、恢复记录）：{scale:?} -> {doubled_scale:?}，失败履约 {failed}"
    );
    for (label, measured) in [("基础规模", &base), ("双倍规模", &doubled)] {
        print_pair(&format!("{label} 水位"), &measured.snapshot_legacy, &measured.snapshot);
        print_pair(&format!("{label} 配对终态"), &measured.paired_legacy, &measured.paired);
        println!(
            "{label} 入口读出行数：修复前 {:?}，现行 {:?} + {:?}；配对终态现行 {:?}",
            measured.snapshot_legacy.index_rows(LEGACY_OPEN_INDEX),
            measured.snapshot.index_rows(OPEN_INDEX),
            measured.snapshot.index_rows(BLOCKED_INDEX),
            measured.paired.index_rows(OPEN_INDEX),
        );
    }

    // 现行水位入口只经两个部分索引读取，不顺序扫描义务或履约；读出行数与缓冲区不随或有退款与
    // 已满足历史增长。修复前的同形索引读出的行数恰好多出新增的或有退款。
    let added_dormant = doubled_scale[1] - scale[1];
    assert!(added_dormant >= 9_000, "追加历史应新增大量或有退款：{added_dormant}");
    for measured in [&base, &doubled] {
        for index in [OPEN_INDEX, BLOCKED_INDEX] {
            assert!(
                measured.snapshot.uses_index(index),
                "水位入口应使用 {index}：{:?}",
                measured.snapshot.node_types()
            );
        }
        for table in ["settlement_obligations", "settlement_obligation_fulfillments"] {
            assert!(
                !seq_scans(&measured.snapshot, table),
                "现行水位不应顺序扫描 {table}：{:?}",
                measured.snapshot.node_types()
            );
            assert!(
                !seq_scans(&measured.paired, table),
                "现行配对口径不应顺序扫描 {table}：{:?}",
                measured.paired.node_types()
            );
        }
        assert!(
            measured.paired.uses_index(OPEN_INDEX),
            "配对口径应只从活动部分索引出发：{:?}",
            measured.paired.node_types()
        );
        assert!(
            measured.snapshot_legacy.uses_index(LEGACY_OPEN_INDEX),
            "修复前水位入口应使用同形索引：{:?}",
            measured.snapshot_legacy.node_types()
        );
        assert!(
            seq_scans(&measured.paired_legacy, "settlement_obligations"),
            "修复前配对口径应从全部义务出发：{:?}",
            measured.paired_legacy.node_types()
        );
    }
    let entry = |measured: &Measured, index: &str| {
        measured.snapshot.index_rows(index).expect("现行入口经部分索引读出行")
    };
    let legacy_entry = |measured: &Measured| {
        measured.snapshot_legacy.index_rows(LEGACY_OPEN_INDEX).expect("修复前入口经同形索引读出行")
    };
    assert_eq!(entry(&base, OPEN_INDEX), entry(&doubled, OPEN_INDEX), "活动履约入口不应增长");
    assert_eq!(entry(&base, BLOCKED_INDEX), entry(&doubled, BLOCKED_INDEX), "blocked 入口不应增长");
    assert_eq!(
        legacy_entry(&base) - entry(&base, OPEN_INDEX),
        scale[1],
        "修复前入口比现行多出的恰是全部或有未激活退款"
    );
    assert_eq!(
        legacy_entry(&doubled) - legacy_entry(&base),
        added_dormant,
        "修复前入口随新增或有退款同步增长"
    );
    assert_eq!(
        base.paired.index_rows(OPEN_INDEX),
        doubled.paired.index_rows(OPEN_INDEX),
        "配对口径入口行数不应随历史增长"
    );
    // 历史翻倍后，现行配对口径的缓冲区只允许小幅波动；水位按活动行逐个主键查找，B-tree 加深一层
    // 时每次查找多读一页（对数增长），翻倍历史只允许增长两成，远低于历史的线性增长。
    for (label, before, after, percent) in [
        ("水位", &base.snapshot, &doubled.snapshot, 120),
        ("配对终态", &base.paired, &doubled.paired, 105),
    ] {
        assert!(
            buffers(after) * 100 <= buffers(before) * percent,
            "现行{label}缓冲区不应随历史线性增长：{} -> {}",
            buffers(before),
            buffers(after)
        );
    }
    assert!(
        buffers(&doubled.paired_legacy) > buffers(&base.paired_legacy),
        "修复前配对口径缓冲区随义务总数增长"
    );

    let summary = |measured: &Measured| {
        json!({
            "snapshot": measured.snapshot.to_json(),
            "snapshot_before_fix": measured.snapshot_legacy.to_json(),
            "paired_terminal": measured.paired.to_json(),
            "paired_terminal_before_fix": measured.paired_legacy.to_json(),
        })
    };
    let report = json!({
        "schema": "auction.alert_fulfillment_plans.v1",
        "scale": BTreeMap::from([("base", scale), ("doubled", doubled_scale)]),
        "scale_columns": ["obligations", "dormant_refunds", "recoveries"],
        "failed_fulfillments": failed,
        "base": summary(&base),
        "doubled": summary(&doubled),
    });
    let path = write_report("alert-fulfillment-plans", &report).expect("持久化告警履约计划报告");
    println!("告警履约计划报告：{}", path.display());
    pool.close().await;
}
