//! 告警履约水位与履约终态配对口径的入口等价：入口只读活动集合，读数必须与不限入口的全表读法
//! 逐列相同。
//!
//! - 水位：生产查询原文与“去掉入口限制、分类全部义务”的全表读法（同一份分类与计数原文）比较，
//!   覆盖从未激活的或有退款（带与不带 `dormant` 标记）、缺履约行的 `ready`/`blocked` 义务、已满足
//!   历史与各分类；另与修复前“活动履约 JOIN 义务预筛业务状态”的入口比较；
//! - 配对口径：生产查询原文与修复前“全部义务 LEFT JOIN 履约”的全表入口比较。
//!
//! 比较函数也被其他用例在各自场景里调用（装备损失、COD 退回、装备收回等终态）。只用直接 SQL
//! 构造事实，被测 SQL 经 `auction_storage::alert_benchmark_sql` 读取生产原文。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration as StdDuration;

use auction_application::operations::{
    OperationsAlertCollectionOverdueRead, OperationsAlertSourceReader,
};
use auction_time::{Duration, Timestamp};
use serde_json::Value;
use sqlx::{AssertSqlSafe, PgPool};
use uuid::Uuid;

use crate::support;
use crate::support::fixtures::{policy_snapshot, seed_user, stable_id};

/// 水位查询的入口限制；换成 `WHERE true` 即不限入口、分类全部义务。
const SNAPSHOT_ENTRY_FILTER: &str =
    "WHERE obligation.id = ANY ((SELECT ids FROM live_ids)::uuid[])";
/// 水位入口第一段的起止标记：从活动履约出发，到并上缺行 blocked 义务为止。
const ENTRY_START: &str = "SELECT open.obligation_id";
const ENTRY_END: &str = "UNION ALL";
/// 修复前的水位入口第一段：活动履约联表义务预筛业务状态。
const LEGACY_ENTRY: &str = "SELECT open.obligation_id
            FROM public.settlement_obligation_fulfillments AS open
            JOIN public.settlement_obligations AS obligation
              ON obligation.id = open.obligation_id
            WHERE open.execution_state <> 'satisfied'
              AND (obligation.business_state IN ('ready', 'blocked')
                   OR open.execution_state IN ('failed', 'manual_reconcile'))
            ";
/// 配对口径的现行扫描入口起点；从这里到语句末尾换成修复前的全表入口。
const PAIRED_SOURCE_START: &str = "FROM public.settlement_obligation_fulfillments AS fulfillment";
/// 修复前的配对口径入口：从全部义务出发，再按履约状态过滤。
const PAIRED_LEGACY_SOURCE: &str = "FROM public.settlement_obligations AS obligation
           LEFT JOIN public.settlement_obligation_fulfillments AS fulfillment
             ON fulfillment.obligation_id = obligation.id";
/// 水位查询的两组实参：读超期与清单（一小时阈值），以及不评估超期。
pub(crate) const SNAPSHOT_ARGUMENTS: [&str; 2] =
    ["now(), now() - interval '1 hour', true, 200", "now(), NULL, false, 200"];

/// 不限入口的水位读法：同一份分类与计数原文，分类全部义务。
pub(crate) fn full_scan_snapshot_sql(sql: &str) -> String {
    assert!(sql.contains(SNAPSHOT_ENTRY_FILTER), "水位查询应按活动集合限定分类");
    sql.replacen(SNAPSHOT_ENTRY_FILTER, "WHERE true", 1)
}

/// 修复前的水位查询原文：入口第一段换回联表预筛。
pub(crate) fn legacy_snapshot_sql(sql: &str) -> String {
    let start = sql.find(ENTRY_START).expect("水位入口应从活动履约出发");
    let end = start + sql[start..].find(ENTRY_END).expect("水位入口应并上缺行 blocked 义务");
    format!("{}{LEGACY_ENTRY}{}", &sql[..start], &sql[end..])
}

/// 修复前的配对口径原文：扫描入口换回全部义务 LEFT JOIN 履约。
pub(crate) fn legacy_paired_sql(sql: &str) -> String {
    let start = sql.find(PAIRED_SOURCE_START).expect("配对口径应从失败履约出发");
    format!("{}{PAIRED_LEGACY_SOURCE}", &sql[..start])
}

/// 把整条语句的结果行转成 JSON，逐列比较；`arguments` 是 `EXECUTE` 实参（空串表示无参数）。
pub(crate) async fn result_row(pool: &PgPool, sql: &str, arguments: &str) -> Value {
    let script = if arguments.is_empty() {
        format!("SELECT to_jsonb(result) FROM ({sql}) AS result")
    } else {
        format!(
            "PREPARE entry_result AS SELECT to_jsonb(result) FROM ({sql}) AS result;
             EXECUTE entry_result({arguments});
             DEALLOCATE entry_result;"
        )
    };
    let rows = sqlx::raw_sql(AssertSqlSafe(script)).fetch_all(pool).await.expect("执行读数查询");
    let row = rows.first().expect("读数查询返回一行");
    sqlx::Row::try_get::<sqlx::types::Json<Value>, _>(row, 0).expect("读数 JSON").0
}

/// 当前数据上，水位与配对口径的生产读数都等于不限入口的全表读法与修复前读法。
pub(crate) async fn assert_alert_reads_match_full_scan(pool: &PgPool) {
    let snapshot = auction_storage::alert_benchmark_sql::fulfillment_snapshot_sql();
    for arguments in SNAPSHOT_ARGUMENTS {
        let production = result_row(pool, snapshot, arguments).await;
        assert_eq!(
            production,
            result_row(pool, &full_scan_snapshot_sql(snapshot), arguments).await,
            "水位读数应与不限入口的全表读法逐列相同（{arguments}）"
        );
        assert_eq!(
            production,
            result_row(pool, &legacy_snapshot_sql(snapshot), arguments).await,
            "水位读数应与修复前入口逐列相同（{arguments}）"
        );
    }
    let paired = auction_storage::alert_benchmark_sql::paired_fulfillment_counts_sql();
    assert_eq!(
        result_row(pool, paired, "").await,
        result_row(pool, &legacy_paired_sql(paired), "").await,
        "配对口径读数应与修复前全表入口逐列相同"
    );
}

const SEED: &str = "alert-entry";

/// 矩阵一行：编号、义务种类、业务状态、履约状态（`none` 为缺行）、或有未激活标记、投递状态、
/// 失败码、前置义务编号、寄出距今小时数。
type Case = (
    i32,
    &'static str,
    &'static str,
    &'static str,
    bool,
    Option<&'static str>,
    Option<&'static str>,
    Option<i32>,
    i64,
);

/// 覆盖入口内外的全部形状：已满足历史、带与不带标记的或有退款、缺行的 ready/blocked 义务、
/// 等待前置、积压、等待领取（含超期）、失败与人工核对。
const CASES: [Case; 20] = [
    (1, "winner_equipment_cod", "ready", "enqueued", false, Some("succeeded"), None, None, 2),
    (2, "winner_equipment_cod", "ready", "satisfied", false, Some("succeeded"), None, None, 48),
    (
        3,
        "winner_equipment_cod",
        "ready",
        "failed",
        false,
        Some("succeeded"),
        Some("collection_evidence_mismatch"),
        None,
        48,
    ),
    (
        4,
        "winner_equipment_cod",
        "ready",
        "manual_reconcile",
        false,
        Some("manual_reconcile"),
        Some("send_outcome_unknown"),
        None,
        48,
    ),
    (5, "seller_payout", "blocked", "pending", false, None, None, Some(1), 0),
    (6, "seller_payout", "blocked", "none", false, None, None, Some(2), 0),
    (7, "seller_payout", "ready", "pending", false, None, None, Some(2), 0),
    (8, "seller_payout", "ready", "enqueued", false, Some("succeeded"), None, Some(2), 2),
    (9, "seller_payout", "ready", "enqueued", false, Some("succeeded"), None, Some(2), 0),
    (10, "seller_payout", "ready", "satisfied", false, Some("succeeded"), None, Some(2), 48),
    (11, "seller_payout", "ready", "failed", false, None, Some("recovery_sale_voided"), Some(2), 0),
    (12, "buyer_refund", "contingent", "pending", true, None, None, Some(2), 0),
    (13, "buyer_refund", "contingent", "pending", false, None, None, Some(2), 0),
    (
        14,
        "buyer_refund",
        "contingent",
        "failed",
        false,
        None,
        Some("operator_external_buyer_compensation"),
        Some(2),
        0,
    ),
    (15, "buyer_refund", "ready", "pending", false, None, None, Some(2), 0),
    (16, "buyer_refund", "ready", "enqueued", false, Some("succeeded"), None, Some(2), 3),
    (17, "unsold_return", "ready", "pending", false, None, None, None, 0),
    (18, "unsold_return", "ready", "prepared", false, Some("pending"), None, None, 0),
    (19, "seller_payout", "ready", "none", false, None, None, Some(2), 0),
    (
        20,
        "buyer_refund",
        "ready",
        "manual_reconcile",
        false,
        Some("manual_reconcile"),
        Some("send_outcome_unknown"),
        Some(2),
        48,
    ),
];

fn case_id(kind: &str, number: i32) -> Uuid {
    stable_id(SEED, &format!("{kind}-{number}"))
}

async fn seed_matrix(pool: &PgPool) {
    let now = Timestamp::now();
    let ended = now - Duration::days(10);
    let auction = stable_id(SEED, "auction");
    let command = stable_id(SEED, "command");
    let buyer = stable_id(SEED, "buyer");
    seed_user(pool, buyer, "alert-entry-buyer").await;
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, 'main', 7, 'alert entry', 'settled', $2, $2, $2, $2, $2, $2, $3)",
    )
    .bind(auction)
    .bind(ended)
    .bind(policy_snapshot(ended, 1_000, 100))
    .execute(pool)
    .await
    .expect("写入已结算场次");
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'alert-entry-settlement', $2, $3, 'accepted')",
    )
    .bind(command)
    .bind(auction)
    .bind([7_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入结算命令");

    for (number, kind, state, execution, dormant, delivery_status, error, prerequisite, hours) in
        CASES
    {
        let lot = case_id("lot", number);
        let equipment = case_id("equipment", number);
        let obligation = case_id("obligation", number);
        sqlx::query(
            "INSERT INTO public.equipment
                (id, world_id, external_equipment_id, name, category, attributes, collected_at)
             VALUES ($1, 'main', $2, $3, 'weapon', '{}'::jsonb, $4)",
        )
        .bind(equipment)
        .bind(format!("{}", 7_700_000 + number))
        .bind(format!("Alert entry equipment {number}"))
        .bind(ended - Duration::days(1))
        .execute(pool)
        .await
        .expect("写入装备");
        sqlx::query(
            "INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
                 effective_end_at, category_key, category_number)
             VALUES ($1, $2, $3, $4, 'sold', 1000, 1000, $5, 'weapon', $4)",
        )
        .bind(lot)
        .bind(auction)
        .bind(equipment)
        .bind(number)
        .bind(ended)
        .execute(pool)
        .await
        .expect("写入 lot");
        let to_user = matches!(kind, "winner_equipment_cod" | "buyer_refund");
        // 前置义务必须与本义务同场次/同 lot/同装备（迁移 000147）；为每个带前置的用例
        // 先写一条同身份的 winner_equipment_cod，作为可满足的前置根。
        let prerequisite_id = prerequisite.map(|_| case_id("obligation-parent", number));
        if let Some(parent) = prerequisite_id {
            sqlx::query(
                "INSERT INTO public.settlement_obligations
                    (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                     obligation_kind, business_state, recipient_user_id, amount, carrier_credit,
                     equipment_id, prerequisite_obligation_id, created_at, updated_at)
                 VALUES ($1, $2, $3, 'main', $4, 1, 'winner_equipment_cod', 'ready', $5,
                         1000, 0, $6, NULL, $7, $7)",
            )
            .bind(parent)
            .bind(auction)
            .bind(lot)
            .bind(command)
            .bind(buyer)
            .bind(equipment)
            .bind(ended)
            .execute(pool)
            .await
            .expect("写入前置义务");
            // 只有被引用用例已满足时才补履约行：缺失履约等于“未满足”，既保持那一例
            // 等待前置的语义，也不会给待办计数凭空加一行。
            let satisfied = prerequisite.is_some_and(|referenced| {
                CASES.iter().any(|case| case.0 == referenced && case.3 == "satisfied")
            });
            if satisfied {
                sqlx::query(
                    "INSERT INTO public.settlement_obligation_fulfillments
                        (obligation_id, execution_state, dispatch_world, delivery_operation,
                         completed_at)
                     VALUES ($1, 'satisfied', 'main', 'send', $2)",
                )
                .bind(parent)
                .bind(ended)
                .execute(pool)
                .await
                .expect("写入前置履约");
            }
        }
        sqlx::query(
            "INSERT INTO public.settlement_obligations
                (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                 obligation_kind, business_state, recipient_user_id, recipient_game_identity,
                 amount, carrier_credit, equipment_id, prerequisite_obligation_id, created_at,
                 updated_at)
             VALUES ($1, $2, $3, 'main', $4, 1, $5, $6, $7, $8, $9, 0, $10, $11, $12, $12)",
        )
        .bind(obligation)
        .bind(auction)
        .bind(lot)
        .bind(command)
        .bind(kind)
        .bind(state)
        .bind(to_user.then_some(buyer))
        .bind((!to_user).then_some("alert-entry-seller"))
        .bind(if kind == "unsold_return" { 0_i64 } else { 1_000 })
        .bind(equipment)
        .bind(prerequisite_id)
        .bind(ended)
        .execute(pool)
        .await
        .expect("写入义务");
        let sent_at = now - Duration::hours(hours) - Duration::minutes(1);
        let delivery = delivery_status.map(|_| case_id("delivery", number));
        if let (Some(delivery), Some(status)) = (delivery, delivery_status) {
            let completed = matches!(status, "succeeded" | "manual_reconcile");
            sqlx::query(
                "INSERT INTO public.deliveries
                    (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                     status, fencing_version, attempt_count, claimed_at, sending_at, completed_at,
                     external_reference, failure_code, result_metadata, created_at, updated_at)
                 VALUES ($1, $2, decode(repeat('e1', 32), 'hex'), 'settlement', $3,
                         'alert-entry-recipient',
                         jsonb_build_object('schema_version', 1, 'operation', 'send',
                             'world', 'main', 'template', 'settlement_alert_entry',
                             'credits', 0, 'cod', 0, 'attachment_ids', '[]'::jsonb),
                         $4, 1, $5, $6, $6, $6, $7, $8, '{}'::jsonb, $9, $9)",
            )
            .bind(delivery)
            .bind(case_id("delivery-command", number))
            .bind(obligation.to_string())
            .bind(status)
            .bind(i32::from(completed))
            .bind(completed.then_some(sent_at))
            .bind(
                (status == "succeeded").then(|| format!("mooglemail:main:send:{}", 5_500 + number)),
            )
            .bind((status == "manual_reconcile").then_some("send_outcome_unknown"))
            .bind(sent_at)
            .execute(pool)
            .await
            .expect("写入投递");
        }
        if execution == "none" {
            continue;
        }
        let terminal = matches!(execution, "satisfied" | "failed" | "manual_reconcile");
        sqlx::query(
            "INSERT INTO public.settlement_obligation_fulfillments
                (obligation_id, execution_state, dispatch_world, delivery_operation, delivery_id,
                 dormant, last_error_code, proof_kind, proof_reference, proof_digest,
                 proof_observed_at, completed_at, created_at, updated_at)
             VALUES ($1, $2, 'main', 'send', $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(obligation)
        .bind(execution)
        .bind(delivery)
        .bind(dormant)
        .bind(error)
        .bind((execution == "satisfied").then_some("mooglemail_sent_collected"))
        .bind((execution == "satisfied").then(|| format!("{}", 6_600 + number)))
        .bind((execution == "satisfied").then_some([9_u8; 32].as_slice()))
        .bind((execution == "satisfied").then_some(now - Duration::minutes(30)))
        .bind(terminal.then_some(now - Duration::minutes(30)))
        .bind(ended)
        .bind(now - Duration::minutes(10))
        .execute(pool)
        .await
        .expect("写入履约");
    }
}

/// 入口内外各种形状下，生产水位与不限入口的全表读法、修复前入口逐列相同；带标记的或有退款不
/// 进入入口，也不改变任何计数。经生产端口读取的计数与矩阵预期一致，保证比较确实覆盖到各分类。
#[tokio::test]
async fn postgres_alert_entry_reads_match_full_scan_across_states() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    seed_matrix(pool).await;

    assert_alert_reads_match_full_scan(pool).await;
    let entry: Vec<Uuid> = sqlx::query_scalar(
        "SELECT obligation_id FROM public.settlement_obligation_fulfillments
         WHERE execution_state <> 'satisfied' AND NOT dormant ORDER BY obligation_id",
    )
    .fetch_all(pool)
    .await
    .expect("读取活动入口");
    assert!(!entry.contains(&case_id("obligation", 12)), "带标记的或有退款不进入活动入口");
    assert!(entry.contains(&case_id("obligation", 13)), "未带标记的或有退款仍在入口内");

    let snapshot = store
        .snapshot(
            Timestamp::now(),
            OperationsAlertCollectionOverdueRead::CountWithItems(StdDuration::from_secs(3_600)),
        )
        .await
        .expect("读取告警快照");
    let fulfillment = snapshot.fulfillment.expect("履约水位");
    assert_eq!(
        (
            fulfillment.pending,
            fulfillment.blocked,
            fulfillment.failed,
            fulfillment.manual_reconcile,
            fulfillment.waiting_prerequisite,
            fulfillment.awaiting_collection,
            fulfillment.collection_overdue,
        ),
        (5, 1, 3, 2, 1, 3, 2),
        "矩阵应覆盖积压、阻塞、失败、人工核对、等待前置、等待领取与超期"
    );
    assert_eq!(
        snapshot.collection_overdue_items.iter().map(|item| item.obligation_id).collect::<Vec<_>>(),
        vec![case_id("obligation", 16), case_id("obligation", 8)],
        "超期清单按寄出最早优先"
    );

    store.postgres_pool().close().await;
}

/// 计划里出现的表与索引名（`Relation Name`/`Index Name`），只解析 `EXPLAIN (FORMAT JSON)`。
fn plan_objects(node: &Value, objects: &mut Vec<String>) {
    for key in ["Relation Name", "Index Name"] {
        if let Some(name) = node.get(key).and_then(Value::as_str) {
            objects.push(name.to_owned());
        }
    }
    for child in node.get("Plans").and_then(Value::as_array).into_iter().flatten() {
        plan_objects(child, objects);
    }
}

/// 在同一连接上禁用顺序扫描后取计划（不执行），列出用到的表与索引；规划器只能在条件蕴含部分
/// 索引谓词时选用该索引。
async fn planned_objects(pool: &PgPool, sql: &str, arguments: &str) -> Vec<String> {
    let execute = if arguments.is_empty() {
        "EXECUTE entry_plan".to_owned()
    } else {
        format!("EXECUTE entry_plan({arguments})")
    };
    let mut connection = pool.acquire().await.expect("取得连接");
    let rows = sqlx::raw_sql(AssertSqlSafe(format!(
        "SET enable_seqscan = off;
         PREPARE entry_plan AS {sql};
         EXPLAIN (FORMAT JSON) {execute};
         DEALLOCATE entry_plan;
         RESET enable_seqscan;"
    )))
    .fetch_all(&mut *connection)
    .await
    .expect("取得计划");
    let plan = rows
        .iter()
        .find_map(|row| sqlx::Row::try_get::<sqlx::types::Json<Value>, _>(row, 0).ok())
        .expect("EXPLAIN 返回 JSON 计划")
        .0;
    let mut objects = Vec::new();
    for entry in plan.as_array().into_iter().flatten() {
        if let Some(root) = entry.get("Plan") {
            plan_objects(root, &mut objects);
        }
    }
    objects
}

/// 入口条件与迁移 000110 的部分索引谓词一致：禁用顺序扫描后，水位入口经活动履约与 blocked 义务
/// 两个部分索引读取；履约终态配对口径只经活动履约索引读取，完全不读义务表（修复前从全部义务
/// LEFT JOIN 履约出发）。
#[tokio::test]
async fn postgres_alert_entries_use_partial_indexes_and_paired_skips_obligations() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    seed_matrix(pool).await;

    let snapshot = planned_objects(
        pool,
        auction_storage::alert_benchmark_sql::fulfillment_snapshot_sql(),
        SNAPSHOT_ARGUMENTS[0],
    )
    .await;
    for index in
        ["settlement_obligation_fulfillments_open_idx", "settlement_obligations_blocked_idx"]
    {
        assert!(snapshot.iter().any(|name| name == index), "水位入口应使用 {index}：{snapshot:?}");
    }
    let paired = planned_objects(
        pool,
        auction_storage::alert_benchmark_sql::paired_fulfillment_counts_sql(),
        "",
    )
    .await;
    assert!(
        paired.iter().any(|name| name == "settlement_obligation_fulfillments_open_idx"),
        "配对口径应只从活动履约索引出发：{paired:?}"
    );
    assert!(
        !paired.iter().any(|name| name == "settlement_obligations"),
        "配对口径不应读取义务表：{paired:?}"
    );

    store.postgres_pool().close().await;
}
