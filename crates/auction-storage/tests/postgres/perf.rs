//! 数据库热点的可重复基准与负载夹具（`test-db` feature，默认不参与 `just test-db`）。
//!
//! 目标：把（同场序列号热点）、（价格历史读路径）、（出价/投递事务内的
//! `pg_notify` 提交串行）从"估计"变成可复现的测量。所有 SQL 都取自生产 owner：
//! 价格历史两条查询经 `auction_storage::benchmark_sql` 读同一份原文，序列号分配与通知
//! 语句在 `benchmark` 模块内复制自 `database/realtime/sequence.rs`、迁移 `000059` 的旧触发器
//! 语义与迁移 `000093` 的提交后唤醒语义，测试里不再另写一份"看起来差不多"的查询。
//!
//! 运行（显式、耗时、需要本机 PostgreSQL 18 二进制）：
//!
//! ```bash
//! cargo test -p auction-storage --features test-db --test postgres -- --ignored perf --nocapture
//! ```
//!
//! 报告写入 `AUCTION_PERF_REPORT_DIR`（默认 `target/perf/<unix 秒>`），包含每个场景的
//! EXPLAIN 摘要、缓冲区、执行时间与并发延迟分位。夹具只写合成数据到一次性克隆库，
//! 不接触真实数据、不启动应用服务、不访问外部网络。
#![cfg(feature = "test-db")]

#[allow(dead_code)]
use crate::support;

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use auction_application::auction::{
    PublicPriceHistoryCursor, PublicPriceHistoryListRequest, PublicPriceHistoryReader,
    PublicPriceHistorySort,
};
use auction_storage::DatabaseStore;
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, PgPool, Row};
use uuid::Uuid;

/// 合成历史的场次数量；每个场次固定 20 件成交装备（全部 Cloth，复用既有分类编号约束）。
const SEED_AUCTIONS: i64 = 500;
const SEED_LOTS_PER_AUCTION: i64 = 20;
const SEED_USERS: i64 = 400;
/// 分页大小与生产默认一致，便于与页面实际请求量比较。
const PAGE_LIMIT: i64 = 25;
/// 价格历史索引名；与迁移 000084 保持同一字符串，索引改名前必须同步此处与断言。
const PRICE_HISTORY_INDEX: &str = "lots_sold_history_effective_end_idx";

/// 价格历史游标页的候选场景（与 Public `/history` 的实际过滤组合一致）。
#[derive(Debug, Clone, Copy)]
enum CursorScenario {
    /// 默认浏览：无过滤、无 boundary。
    FirstPage,
    /// 第二页：带上一页末行的 boundary 条件。
    BoundaryPage,
    /// 只看一个世界。
    WorldFilter,
    /// 价格区间过滤。
    PriceRange,
}

impl CursorScenario {
    const ALL: [Self; 4] =
        [Self::FirstPage, Self::BoundaryPage, Self::WorldFilter, Self::PriceRange];

    const fn label(self) -> &'static str {
        match self {
            Self::FirstPage => "cursor_first_page",
            Self::BoundaryPage => "cursor_boundary_page",
            Self::WorldFilter => "cursor_world_filter",
            Self::PriceRange => "cursor_price_range",
        }
    }
}

/// 编号页（首页带全量中位价统计）的两种排序。
#[derive(Debug, Clone, Copy)]
enum NumberedScenario {
    ClosedDesc,
    PriceDesc,
}

impl NumberedScenario {
    const ALL: [Self; 2] = [Self::ClosedDesc, Self::PriceDesc];

    const fn label(self) -> &'static str {
        match self {
            Self::ClosedDesc => "numbered_closed_desc",
            Self::PriceDesc => "numbered_price_desc",
        }
    }

    const fn sort(self) -> &'static str {
        match self {
            Self::ClosedDesc => "closed_desc",
            Self::PriceDesc => "price_desc",
        }
    }
}

/// 一次 `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)` 的摘要；只保留可比较的数字与节点名。
#[derive(Debug, Clone)]
struct PlanSummary {
    execution_ms: f64,
    planning_ms: f64,
    shared_hit_blocks: i64,
    shared_read_blocks: i64,
    rows_returned: i64,
    node_types: Vec<String>,
}

impl PlanSummary {
    fn uses_index(&self, index: &str) -> bool {
        self.node_types.iter().any(|node| node.contains(index))
    }

    fn to_json(&self) -> Value {
        json!({
            "execution_ms": self.execution_ms,
            "planning_ms": self.planning_ms,
            "shared_hit_blocks": self.shared_hit_blocks,
            "shared_read_blocks": self.shared_read_blocks,
            "rows_returned": self.rows_returned,
            "node_types": self.node_types,
        })
    }
}

/// 并发场景的延迟与冲突统计；全部来自本次运行的真实事务结果。
#[derive(Debug, Default, Clone)]
struct LoadStats {
    transactions: usize,
    attempts: BTreeMap<usize, usize>,
    conflicts: usize,
    exhausted: usize,
    other_failures: usize,
    latencies_micros: Vec<u64>,
    commit_micros: Vec<u64>,
    /// 提交后单独发送唤醒的耗时（含取连接与自动提交）；不并入 commit_micros。
    post_commit_notify_micros: Vec<u64>,
}

impl LoadStats {
    fn merge(&mut self, other: Self) {
        self.transactions += other.transactions;
        for (attempt, count) in other.attempts {
            *self.attempts.entry(attempt).or_default() += count;
        }
        self.conflicts += other.conflicts;
        self.exhausted += other.exhausted;
        self.other_failures += other.other_failures;
        self.latencies_micros.extend(other.latencies_micros);
        self.commit_micros.extend(other.commit_micros);
        self.post_commit_notify_micros.extend(other.post_commit_notify_micros);
    }

    fn percentile_micros(&self, percentile: f64) -> u64 {
        percentile_of(&self.latencies_micros, percentile)
    }

    fn commit_percentile_micros(&self, percentile: f64) -> u64 {
        percentile_of(&self.commit_micros, percentile)
    }

    fn post_commit_notify_percentile_micros(&self, percentile: f64) -> u64 {
        percentile_of(&self.post_commit_notify_micros, percentile)
    }

    fn to_json(&self) -> Value {
        json!({
            "transactions": self.transactions,
            "attempts": self.attempts,
            "conflicts": self.conflicts,
            "exhausted": self.exhausted,
            "other_failures": self.other_failures,
            "latency_p50_us": self.percentile_micros(0.50),
            "latency_p95_us": self.percentile_micros(0.95),
            "latency_max_us": self.latencies_micros.iter().copied().max().unwrap_or(0),
            "attempts_total": self.transactions,
            "commit_p50_us": self.commit_percentile_micros(0.50),
            "commit_p95_us": self.commit_percentile_micros(0.95),
            "commit_max_us": self.commit_micros.iter().copied().max().unwrap_or(0),
            "post_commit_notify_p50_us":
                self.post_commit_notify_percentile_micros(0.50),
            "post_commit_notify_p95_us":
                self.post_commit_notify_percentile_micros(0.95),
            "post_commit_notify_max_us":
                self.post_commit_notify_micros.iter().copied().max().unwrap_or(0),
        })
    }
}

fn percentile_of(samples: &[u64], percentile: f64) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let index = ((sorted.len() as f64 - 1.0) * percentile).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn report_dir() -> PathBuf {
    std::env::var_os("AUCTION_PERF_REPORT_DIR").map_or_else(
        || {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs());
            PathBuf::from(format!("target/perf/{stamp}"))
        },
        PathBuf::from,
    )
}

fn write_report(name: &str, report: &Value) -> PathBuf {
    let directory = report_dir();
    if let Err(error) = std::fs::create_dir_all(&directory) {
        println!("无法创建报告目录 {}：{error}", directory.display());
        return directory;
    }
    let path = directory.join(format!("{name}.json"));
    match serde_json::to_string_pretty(report) {
        Ok(rendered) => {
            if let Err(error) = std::fs::write(&path, rendered) {
                println!("无法写入报告 {}：{error}", path.display());
            }
        }
        Err(error) => println!("无法序列化报告：{error}"),
    }
    path
}

/// 写入合成历史：只有已结算场次、已成交 lot、买家与逐 lot 结算，形状与真实投影一致。
async fn seed(pool: &PgPool) -> Result<(), String> {
    let statements: Vec<(&str, Vec<i64>)> = vec![
        (
            "INSERT INTO public.users (id, username, public_key_hash)
             SELECT gen_random_uuid(), 'perf-buyer' || g,
                    decode(md5('perf-buyer' || g) || md5('perf-key' || g), 'hex')
             FROM generate_series(1, $1) AS g",
            vec![SEED_USERS],
        ),
        (
            "INSERT INTO public.auction_commands (command_id, command_kind, aggregate_id, request_hash, result_code)
             SELECT gen_random_uuid(), 'settle_auction', gen_random_uuid(),
                    decode(md5('perf-cmd' || g) || md5('perf-cmd2' || g), 'hex'), 'accepted'
             FROM generate_series(1, $1) AS g",
            vec![SEED_AUCTIONS],
        ),
        (
            "INSERT INTO public.auctions (id, world_id, sequence, title, status, settled_at, scheduled_open_at,
                                          preview_scheduled_at, scheduled_end_at, effective_end_at, policy_snapshot)
             SELECT gen_random_uuid(), CASE WHEN g % 2 = 0 THEN 'main' ELSE 'isekai' END, g,
                    'Perf auction ' || g, 'settled', now() - (g || ' days')::interval,
                    now() - ((g + 10) || ' days')::interval, now() - ((g + 12) || ' days')::interval,
                    now() - ((g + 1) || ' days')::interval, now() - (g || ' days')::interval,
                    jsonb_build_object(
                      'schema_version', 4, 'start_price', 50000, 'bid_increment_ppm', 0,
                      'minimum_increment', 30000, 'soft_close_window_seconds', 900,
                      'soft_close_extension_seconds', 900, 'proxy_enabled', true,
                      'proxy_default_mode', 'fast', 'proxy_slow_interval_seconds', 600,
                      'proxy_max_outbid_uses', 0, 'final_phase_lead_seconds', 3600,
                      'scheduled_end_at',
                      to_char(now() - ((g + 1) || ' days')::interval, 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'),
                      'settlement', jsonb_build_object(
                        'version', 1, 'mode', 'hold_curve_v1', 'compensation_pool_enabled', true,
                        'isekai_carrier_credit', 1,
                        'anchors', jsonb_build_array(
                          jsonb_build_object('hold_ppm', 0, 'adjustment_ppm', 0),
                          jsonb_build_object('hold_ppm', 1000000, 'adjustment_ppm', 0))),
                      'recovery', jsonb_build_object(
                        'version', 1, 'mode', 'remind_v1', 'payment_timeout_seconds', 604800,
                        'reminder_limit', 1))
             FROM generate_series(1, $1) AS g",
            vec![SEED_AUCTIONS],
        ),
        (
            "INSERT INTO public.equipment (id, world_id, external_equipment_id, name, category, attributes, collected_at)
             SELECT gen_random_uuid(), a.world_id, 'perf-' || a.sequence || '-' || n,
                    (ARRAY['Legendary Radiant Phase Shoes of Niflheim', 'Peerless Ethereal Axe of the Void',
                           'Exquisite Mystic Robe of Flames', 'Superior Arctic Shield of Frost'])[1 + (n % 4)]
                      || ' #' || a.sequence || '-' || n,
                    'Cloth', jsonb_build_object('level', 300 + (n % 50)), now() - '30 days'::interval
             FROM public.auctions a, generate_series(1, $1) AS n
             WHERE a.title LIKE 'Perf auction %'",
            vec![SEED_LOTS_PER_AUCTION],
        ),
        (
            "INSERT INTO public.lots (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
                                      leader_user_id, effective_end_at, category_key, category_number)
             SELECT gen_random_uuid(), a.id, e.id, (split_part(e.external_equipment_id, '-', 3))::integer,
                    'sold', 50000,
                    50000 + (split_part(e.external_equipment_id, '-', 3))::integer * 1000
                      + (a.sequence % 50) * 100,
                    u.id, now() - (a.sequence || ' days')::interval
                      - ((split_part(e.external_equipment_id, '-', 3))::integer || ' hours')::interval,
                    'Cloth', (split_part(e.external_equipment_id, '-', 3))::integer
             FROM public.auctions a
             JOIN public.equipment e ON e.external_equipment_id LIKE 'perf-' || a.sequence || '-%'
             JOIN public.users u
               ON u.username = 'perf-buyer'
                  || (1 + ((a.sequence * 20
                     + (split_part(e.external_equipment_id, '-', 3))::integer) % $1))
             WHERE a.title LIKE 'Perf auction %'",
            vec![SEED_USERS],
        ),
        (
            "INSERT INTO public.lot_settlements (lot_id, end_price, listed_price, buyer_amount, seller_amount,
                                                tax_amount, compensation_amount, hold_ppm, adjustment_ppm)
             SELECT l.id, l.current_price, l.current_price,
                    l.current_price + greatest(1, l.current_price / 20), l.current_price,
                    greatest(1, l.current_price / 20), 0, 0, 0
             FROM public.lots l",
            vec![],
        ),
        (
            "INSERT INTO public.auction_settlements (auction_id, command_id, policy_version, policy_mode,
                                                    pool_enabled, opening_pool_balance, closing_pool_balance,
                                                    tax_total, settled_at)
             SELECT a.id, c.command_id, 1, 'hold_curve_v1', true, 0, 0, 0, a.settled_at
             FROM public.auctions a
             JOIN (
               SELECT command_id, row_number() OVER (ORDER BY command_id) AS rn
               FROM public.auction_commands WHERE command_kind = 'settle_auction'
             ) c ON c.rn = a.sequence
             WHERE a.title LIKE 'Perf auction %'",
            vec![],
        ),
        ("ANALYZE public.lots", vec![]),
        ("ANALYZE public.auctions", vec![]),
        ("ANALYZE public.equipment", vec![]),
        ("ANALYZE public.users", vec![]),
        ("ANALYZE public.lot_settlements", vec![]),
        ("ANALYZE public.auction_settlements", vec![]),
    ];

    for (sql, values) in statements {
        let mut query = sqlx::query(sql);
        for value in values {
            query = query.bind(value);
        }
        query
            .execute(pool)
            .await
            .map_err(|error| format!("合成历史写入失败：{error}\nSQL: {sql}"))?;
    }
    Ok(())
}

/// 用 `PREPARE` + `EXPLAIN` 测量生产查询原文；参数只用受控的合成字面量，避免另写一份 SQL。
async fn explain(pool: &PgPool, sql: &str, arguments: &str) -> Result<PlanSummary, String> {
    let script = format!(
        "PREPARE perf_probe AS {sql};\nEXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) EXECUTE perf_probe({arguments});\nDEALLOCATE perf_probe;"
    );
    let rows = sqlx::raw_sql(AssertSqlSafe(script))
        .fetch_all(pool)
        .await
        .map_err(|error| format!("EXPLAIN 失败：{error}"))?;
    // 多语句脚本按顺序返回三个结果集的最后一行（EXPLAIN 的 JSON 列）。
    let mut plan: Option<Value> = None;
    for row in &rows {
        if let Ok(sqlx::types::Json(value)) = row.try_get::<sqlx::types::Json<Value>, _>(0) {
            plan = Some(value);
        }
    }
    let plan = plan.ok_or_else(|| "EXPLAIN 未返回 JSON 计划".to_owned())?;
    let entry = plan
        .as_array()
        .and_then(|items| items.first())
        .ok_or_else(|| "EXPLAIN 结果为空".to_owned())?;

    let mut summary = PlanSummary {
        execution_ms: entry.get("Execution Time").and_then(Value::as_f64).unwrap_or(0.0),
        planning_ms: entry.get("Planning Time").and_then(Value::as_f64).unwrap_or(0.0),
        shared_hit_blocks: 0,
        shared_read_blocks: 0,
        rows_returned: 0,
        node_types: Vec::new(),
    };
    if let Some(root) = entry.get("Plan") {
        collect_plan(root, &mut summary, true);
    }
    Ok(summary)
}

fn collect_plan(node: &Value, summary: &mut PlanSummary, is_root: bool) {
    if let Some(node_type) = node.get("Node Type").and_then(Value::as_str) {
        let relation = node.get("Index Name").and_then(Value::as_str);
        summary.node_types.push(match relation {
            Some(index) => format!("{node_type}({index})"),
            None => node_type.to_owned(),
        });
    }
    summary.shared_hit_blocks += node.get("Shared Hit Blocks").and_then(Value::as_i64).unwrap_or(0);
    summary.shared_read_blocks +=
        node.get("Shared Read Blocks").and_then(Value::as_i64).unwrap_or(0);
    if is_root {
        summary.rows_returned = node.get("Actual Rows").and_then(Value::as_i64).unwrap_or(0);
    }
    if let Some(children) = node.get("Plans").and_then(Value::as_array) {
        for child in children {
            collect_plan(child, summary, false);
        }
    }
}

/// 游标页的参数：`$1` 名称、`$2` 世界、`$3/$4` 成交时间、`$5/$6` 价格、`$7` 买家、
/// `$8` 卖家、`$9` boundary、`$10..$13` boundary 值、`$14` limit。
fn cursor_arguments(
    scenario: CursorScenario,
    boundary: Option<&PublicPriceHistoryEntryRow>,
) -> String {
    let boundary = match scenario {
        CursorScenario::BoundaryPage => boundary,
        _ => None,
    };
    let (world, minimum, maximum, closed_from, closed_before) = match scenario {
        CursorScenario::WorldFilter => ("'main'", "NULL", "NULL", "NULL", "NULL"),
        CursorScenario::PriceRange => ("NULL", "200000", "400000", "NULL", "NULL"),
        _ => ("NULL", "NULL", "NULL", "NULL", "NULL"),
    };
    let (use_boundary, closed_at, sequence, lot_number, lot_id) = boundary.map_or_else(
        || ("false", "NULL".to_owned(), "NULL".to_owned(), "NULL".to_owned(), "NULL".to_owned()),
        |row| {
            (
                "true",
                format!("'{}'::timestamptz", row.closed_at),
                row.auction_sequence.to_string(),
                row.ordering_lot_number.to_string(),
                format!("'{}'::uuid", row.lot_id),
            )
        },
    );
    format!(
        "NULL, {world}, {closed_from}, {closed_before}, {minimum}, {maximum}, NULL, NULL, \
         {use_boundary}, {closed_at}, {sequence}, {lot_number}, {lot_id}, {}",
        PAGE_LIMIT + 1
    )
}

/// 编号页的参数：`$1` 名称、`$2` 世界、`$3/$4` 成交时间、`$5/$6` 价格、`$7` 买家、
/// `$8` 卖家、`$9` limit、`$10` offset、`$11` 排序。
fn numbered_arguments(sort: &str) -> String {
    format!("NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, {PAGE_LIMIT}, 0, '{sort}'")
}

/// 需要 boundary 值的那一行；直接取生产查询第一页的最后一行，避免测试另造身份。
struct PublicPriceHistoryEntryRow {
    lot_id: String,
    auction_sequence: i64,
    ordering_lot_number: i64,
    closed_at: String,
}

async fn first_page_tail(store: &DatabaseStore) -> Result<PublicPriceHistoryEntryRow, String> {
    let page = store
        .list_public_price_history(cursor_request(None))
        .await
        .map_err(|error| format!("读取价格历史首屏失败：{error}"))?;
    let last = page.entries.last().ok_or_else(|| "合成历史为空，无法取 boundary".to_owned())?;
    Ok(PublicPriceHistoryEntryRow {
        lot_id: last.lot_id.to_string(),
        auction_sequence: last.auction_sequence,
        ordering_lot_number: last.ordering_lot_number,
        closed_at: last.closed_at.format("%Y-%m-%d %H:%M:%S%.6f%:z").to_string(),
    })
}

fn cursor_request(cursor: Option<PublicPriceHistoryCursor>) -> PublicPriceHistoryListRequest {
    PublicPriceHistoryListRequest {
        name_terms: Vec::new(),
        world_id: None,
        closed_from: None,
        closed_before: None,
        minimum_price: None,
        maximum_price: None,
        buyer: None,
        seller: None,
        sort: PublicPriceHistorySort::ClosedDesc,
        limit: PAGE_LIMIT,
        offset: 0,
        cursor,
    }
}

#[tokio::test]
#[ignore = "W12 基准：显式运行，不在默认 test-db 门禁中执行"]
async fn perf_price_history_index_and_plans() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let pool = store.postgres_pool().clone();
    seed(&pool).await.expect("写入合成历史");

    let tail = first_page_tail(&store).await.expect("取 boundary 行");
    let lots: i64 = sqlx::query_scalar("SELECT count(*) FROM public.lots")
        .fetch_one(&pool)
        .await
        .expect("统计合成 lot");
    assert_eq!(lots, SEED_AUCTIONS * SEED_LOTS_PER_AUCTION, "合成历史规模应与配置一致");

    let mut scenarios = BTreeMap::new();
    let mut baseline = BTreeMap::new();

    // 有索引：生产查询原文的计划与结果。
    let cursor_sql = auction_storage::benchmark_sql::cursor_query_sql();
    let numbered_sql = auction_storage::benchmark_sql::numbered_query_sql();
    for scenario in CursorScenario::ALL {
        let summary = explain(&pool, cursor_sql, &cursor_arguments(scenario, Some(&tail)))
            .await
            .expect("游标页 EXPLAIN");
        scenarios.insert(scenario.label().to_owned(), summary);
    }
    for scenario in NumberedScenario::ALL {
        let summary = explain(&pool, numbered_sql, &numbered_arguments(scenario.sort()))
            .await
            .expect("编号页 EXPLAIN");
        scenarios.insert(scenario.label().to_owned(), summary);
    }
    let indexed_page =
        store.list_public_price_history(cursor_request(None)).await.expect("索引版价格历史");

    // 去掉索引再测同一批查询，得到可控的前后对比（只在这份一次性克隆库里执行）。
    sqlx::query(AssertSqlSafe(format!("DROP INDEX {PRICE_HISTORY_INDEX}")))
        .execute(&pool)
        .await
        .expect("删除价格历史索引以取基线");
    sqlx::query("ANALYZE public.lots").execute(&pool).await.expect("重新统计 lot");
    for scenario in CursorScenario::ALL {
        let summary = explain(&pool, cursor_sql, &cursor_arguments(scenario, Some(&tail)))
            .await
            .expect("游标页基线 EXPLAIN");
        baseline.insert(scenario.label().to_owned(), summary);
    }
    for scenario in NumberedScenario::ALL {
        let summary = explain(&pool, numbered_sql, &numbered_arguments(scenario.sort()))
            .await
            .expect("编号页基线 EXPLAIN");
        baseline.insert(scenario.label().to_owned(), summary);
    }
    let unindexed_page =
        store.list_public_price_history(cursor_request(None)).await.expect("无索引价格历史");
    let results_match = indexed_page.entries.len() == unindexed_page.entries.len()
        && indexed_page.entries.iter().zip(unindexed_page.entries.iter()).all(|(left, right)| {
            left.lot_id == right.lot_id
                && left.price == right.price
                && left.closed_at == right.closed_at
                && left.buyer_name == right.buyer_name
                && left.seller_name == right.seller_name
        });

    // 恢复索引，供后续场景与并发的读路径使用。
    sqlx::query(AssertSqlSafe(format!(
        "CREATE INDEX {PRICE_HISTORY_INDEX}
             ON public.lots (effective_end_at DESC, id)
             INCLUDE (auction_id, current_price, lot_number)
             WHERE status = 'sold' AND leader_user_id IS NOT NULL"
    )))
    .execute(&pool)
    .await
    .expect("恢复价格历史索引");
    sqlx::query("ANALYZE public.lots").execute(&pool).await.expect("恢复后统计 lot");

    println!(
        "价格历史：{} 条已成交 lot；索引版首屏 {} 行、无索引 {} 行",
        lots,
        indexed_page.entries.len(),
        unindexed_page.entries.len()
    );
    for (label, summary) in &scenarios {
        let before = baseline.get(label).expect("基线计划");
        println!(
            "{label}: {:.3}ms -> {:.3}ms, buffers {} -> {}",
            before.execution_ms,
            summary.execution_ms,
            before.shared_hit_blocks + before.shared_read_blocks,
            summary.shared_hit_blocks + summary.shared_read_blocks
        );
    }

    // 断言：结果不因索引变化；默认浏览与 boundary 页都改走新索引；缓冲区显著下降。
    assert!(results_match, "索引不得改变价格历史的可见集合与投影");
    let first_page = scenarios.get(CursorScenario::FirstPage.label()).expect("首屏计划");
    let first_page_baseline = baseline.get(CursorScenario::FirstPage.label()).expect("首屏基线");
    assert!(
        first_page.uses_index(PRICE_HISTORY_INDEX),
        "首屏应使用价格历史索引，实际节点：{:?}",
        first_page.node_types
    );
    assert!(!first_page_baseline.uses_index(PRICE_HISTORY_INDEX), "基线计划不应引用被删除的索引");
    let indexed_blocks = first_page.shared_hit_blocks + first_page.shared_read_blocks;
    let baseline_blocks =
        first_page_baseline.shared_hit_blocks + first_page_baseline.shared_read_blocks;
    assert!(
        indexed_blocks * 2 <= baseline_blocks,
        "索引应至少减半首屏缓冲区：{indexed_blocks} vs {baseline_blocks}"
    );
    let boundary_page = scenarios.get(CursorScenario::BoundaryPage.label()).expect("boundary 计划");
    assert!(
        boundary_page.uses_index(PRICE_HISTORY_INDEX),
        "boundary 页应使用价格历史索引，实际节点：{:?}",
        boundary_page.node_types
    );

    let report = json!({
        "schema": "auction.price_history_plan.v1",
        "seeded_lots": lots,
        "page_limit": PAGE_LIMIT,
        "index": PRICE_HISTORY_INDEX,
        "with_index": scenarios.iter().map(|(label, summary)| (label.clone(), summary.to_json())).collect::<BTreeMap<_, _>>(),
        "without_index": baseline.iter().map(|(label, summary)| (label.clone(), summary.to_json())).collect::<BTreeMap<_, _>>(),
        "results_identical": results_match,
    });
    let path = write_report("price-history-plans", &report);
    println!("价格历史计划报告：{}", path.display());
    pool.close().await;
}

/// 唤醒通知的发送位置：决定 NOTIFY 是否落在业务事务的提交路径上。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotifyMode {
    /// 不发送通知（对照基线）。
    None,
    /// 旧路径：投递写入事务内 `pg_notify`，提交持有 NOTIFY 全局锁。
    InTransaction,
    /// 新路径：事务提交后单独执行自动提交的 `pg_notify`。
    AfterCommit,
}

/// 竞价热点负载：同一场次不同 lot 的 Serializable 事务同时递增同一 `realtime_streams` 行，
/// 复现（同场重叠出价必有一方序列化失败）。
async fn run_contention(
    pool: &PgPool,
    auction_id: &str,
    lot_ids: Arc<Vec<String>>,
    concurrency: usize,
    per_task: usize,
    backoff: bool,
    notify: NotifyMode,
) -> Result<LoadStats, String> {
    let stream_key = format!("auction:{auction_id}");
    let mut handles = Vec::with_capacity(concurrency);
    for task in 0..concurrency {
        let pool = pool.clone();
        let stream_key = stream_key.clone();
        let lot_ids = Arc::clone(&lot_ids);
        handles.push(tokio::spawn(async move {
            let mut stats = LoadStats::default();
            for step in 0..per_task {
                let lot_id = lot_ids[(task * per_task + step) % lot_ids.len()].clone();
                let started = Instant::now();
                let mut attempt = 1_u8;
                loop {
                    stats.transactions += 1;
                    let mut transaction = match pool.begin().await {
                        Ok(transaction) => transaction,
                        Err(_) => {
                            stats.other_failures += 1;
                            break;
                        }
                    };
                    // 与竞价事务相同的语句顺序：先开 Serializable，再分配流序号，再改 lot。
                    // 旧路径在事务内 pg_notify（模拟已删除的 notify_delivery_ready 触发器）；
                    // 新路径在事务提交后单独发送，提交不持有 NOTIFY 全局锁。
                    // PostgreSQL 的 SSI 冲突既可能在语句处报告，也可能延迟到 COMMIT，两个位置
                    // 都必须按同一冲突谓词归类，否则夹具会把真实冲突误记为其它失败。
                    let outcome = apply_hotspot_transaction(
                        &mut transaction,
                        &stream_key,
                        &lot_id,
                        matches!(notify, NotifyMode::InTransaction),
                    )
                    .await;
                    let commit_started = Instant::now();
                    let result = match outcome {
                        Ok(()) => transaction.commit().await.map(|()| commit_started.elapsed()),
                        Err(error) => {
                            let _ = transaction.rollback().await;
                            Err(error)
                        }
                    };
                    match result {
                        Ok(commit_elapsed) => {
                            stats.commit_micros.push(
                                u64::try_from(commit_elapsed.as_micros()).unwrap_or(u64::MAX),
                            );
                            if notify == NotifyMode::AfterCommit {
                                // 提交后再发送；独立自动提交事务，不延长原事务提交。
                                let notify_started = Instant::now();
                                let _ = sqlx::query("SELECT public.signal_delivery_ready()")
                                    .execute(&pool)
                                    .await;
                                stats.post_commit_notify_micros.push(
                                    u64::try_from(notify_started.elapsed().as_micros())
                                        .unwrap_or(u64::MAX),
                                );
                            }
                            stats.latencies_micros.push(
                                u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                            );
                            *stats.attempts.entry(usize::from(attempt)).or_default() += 1;
                            break;
                        }
                        Err(error) => {
                            let conflict = error
                                .as_database_error()
                                .and_then(|database| database.code())
                                .is_some_and(|code| matches!(code.as_ref(), "40001" | "40P01"));
                            if !conflict {
                                stats.other_failures += 1;
                                break;
                            }
                            stats.conflicts += 1;
                            if attempt >= 3 {
                                stats.exhausted += 1;
                                stats.latencies_micros.push(
                                    u64::try_from(started.elapsed().as_micros())
                                        .unwrap_or(u64::MAX),
                                );
                                break;
                            }
                            if backoff {
                                tokio::time::sleep(retry_window(attempt)).await;
                            }
                            attempt += 1;
                        }
                    }
                }
            }
            stats
        }));
    }

    let mut merged = LoadStats::default();
    for handle in handles {
        merged.merge(handle.await.map_err(|error| format!("负载任务失败：{error}"))?);
    }
    Ok(merged)
}

/// 之后的热点事务：Serializable 内只写事件行（`stream_sequence` 为 NULL）与自己的 lot 行，
/// 提交后再由分配器补号。与 `run_contention` 使用同一数据集、并发和重试预算，只替换序号路径，
/// 因此可以直接比较“提交内序号”与“提交后序号”的冲突/耗尽/延迟。
async fn run_contention_deferred(
    pool: &PgPool,
    auction_id: &str,
    lot_ids: Arc<Vec<String>>,
    concurrency: usize,
    per_task: usize,
) -> Result<LoadStats, String> {
    let stream_key = format!("auction:{auction_id}");
    let allocation_sql = auction_storage::realtime_benchmark_sql::deferred_allocation_sql();
    let mut handles = Vec::with_capacity(concurrency);
    for task in 0..concurrency {
        let pool = pool.clone();
        let stream_key = stream_key.clone();
        let lot_ids = Arc::clone(&lot_ids);
        handles.push(tokio::spawn(async move {
            let mut stats = LoadStats::default();
            for step in 0..per_task {
                let lot_id = lot_ids[(task * per_task + step) % lot_ids.len()].clone();
                let started = Instant::now();
                let mut attempt = 1_u8;
                loop {
                    stats.transactions += 1;
                    let mut transaction = match pool.begin().await {
                        Ok(transaction) => transaction,
                        Err(_) => {
                            stats.other_failures += 1;
                            break;
                        }
                    };
                    let outcome =
                        apply_deferred_hotspot_transaction(&mut transaction, &stream_key, &lot_id)
                            .await;
                    let result = match outcome {
                        Ok(()) => {
                            let commit_started = Instant::now();
                            transaction.commit().await.map(|()| commit_started.elapsed())
                        }
                        Err(error) => {
                            let _ = transaction.rollback().await;
                            Err(error)
                        }
                    };
                    match result {
                        Ok(commit_elapsed) => {
                            // 提交后补号：真实路径在 runner 内完成，失败由 relay 兜底。
                            let _ = sqlx::query(allocation_sql)
                                .bind(&stream_key)
                                .bind(256_i64)
                                .execute(&pool)
                                .await;
                            stats.commit_micros.push(
                                u64::try_from(commit_elapsed.as_micros()).unwrap_or(u64::MAX),
                            );
                            stats.latencies_micros.push(
                                u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                            );
                            *stats.attempts.entry(usize::from(attempt)).or_default() += 1;
                            break;
                        }
                        Err(error) => {
                            let conflict = error
                                .as_database_error()
                                .and_then(|database| database.code())
                                .is_some_and(|code| matches!(code.as_ref(), "40001" | "40P01"));
                            if !conflict {
                                stats.other_failures += 1;
                                break;
                            }
                            stats.conflicts += 1;
                            if attempt >= 3 {
                                stats.exhausted += 1;
                                stats.latencies_micros.push(
                                    u64::try_from(started.elapsed().as_micros())
                                        .unwrap_or(u64::MAX),
                                );
                                break;
                            }
                            tokio::time::sleep(retry_window(attempt)).await;
                            attempt += 1;
                        }
                    }
                }
            }
            stats
        }));
    }

    let mut merged = LoadStats::default();
    for handle in handles {
        merged.merge(handle.await.map_err(|error| format!("负载任务失败：{error}"))?);
    }
    Ok(merged)
}

/// 事务形状：事件行（序号 NULL）→ 自己的 lot 行；不触碰共享的 stream 计数行。
async fn apply_deferred_hotspot_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    stream_key: &str,
    lot_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").execute(&mut **transaction).await?;
    sqlx::query(
        "INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload)
         VALUES (gen_random_uuid(), $1, NULL, 'auction.opened', 1,
                 '{\"auction_id\": null}'::jsonb)",
    )
    .bind(stream_key)
    .execute(&mut **transaction)
    .await?;
    sqlx::query("UPDATE public.lots SET state_version = state_version + 1 WHERE id = $1::uuid")
        .bind(lot_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

/// 一次热点事务的全部语句：流序号分配 → lot 行更新 →（可选）同事务 `pg_notify`。
/// 语句顺序与 `database/competition/postgres/bidding` 的真实出价事务一致，只是把事实写入
/// 换成 `state_version` 递增，避免夹具伪造出价金额或资格判定。
async fn apply_hotspot_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    stream_key: &str,
    lot_id: &str,
    notify: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").execute(&mut **transaction).await?;
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO public.realtime_streams AS stream (stream_key, next_sequence, updated_at)
         VALUES ($1, 2, CURRENT_TIMESTAMP)
         ON CONFLICT (stream_key) DO UPDATE
         SET next_sequence = stream.next_sequence + 1,
             updated_at = CURRENT_TIMESTAMP
         RETURNING stream.next_sequence - 1",
    )
    .bind(stream_key)
    .fetch_one(&mut **transaction)
    .await?;
    sqlx::query("UPDATE public.lots SET state_version = state_version + 1 WHERE id = $1::uuid")
        .bind(lot_id)
        .execute(&mut **transaction)
        .await?;
    if notify {
        sqlx::query("SELECT pg_notify('auction_delivery_ready', '')")
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

/// 一次匿名读突发的样本与失败数。
struct TimedReads {
    latencies: Vec<u64>,
    failures: usize,
}

impl TimedReads {
    fn p95_micros(&self) -> u64 {
        percentile_of(&self.latencies, 0.95)
    }
}

/// 与 `run_contention` 并行执行的匿名读突发：`rounds` 轮、每轮 `concurrency` 个并发请求，
/// 全部读取生产价格历史游标页（与浏览器首屏同一查询）。
async fn timed_read_burst(
    store: &Arc<DatabaseStore>,
    pool: &PgPool,
    auction_id: &str,
    lot_ids: Arc<Vec<String>>,
    concurrency: usize,
    rounds: usize,
) -> Result<(TimedReads, LoadStats), String> {
    let store_for_reads = Arc::clone(store);
    let read_handle = tokio::spawn(async move {
        let mut latencies = Vec::with_capacity(concurrency * rounds);
        let mut failures = 0_usize;
        for _ in 0..rounds {
            let mut handles = Vec::with_capacity(concurrency);
            for _ in 0..concurrency {
                let store = Arc::clone(&store_for_reads);
                handles.push(tokio::spawn(async move {
                    let started = Instant::now();
                    let outcome = store.list_public_price_history(cursor_request(None)).await;
                    (
                        outcome.is_ok(),
                        u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                    )
                }));
            }
            for handle in handles {
                let (ok, micros) = handle.await.map_err(|error| format!("读任务失败：{error}"))?;
                if !ok {
                    failures += 1;
                }
                latencies.push(micros);
            }
        }
        Ok::<TimedReads, String>(TimedReads { latencies, failures })
    });

    let bids = run_contention(pool, auction_id, lot_ids, 16, 1, true, NotifyMode::None).await?;
    let reads = read_handle.await.map_err(|error| format!("读突发任务失败：{error}"))??;
    Ok((reads, bids))
}

/// 与 `auction_storage` 退避策略同形的抖动窗口（只用于夹具对照，不参与生产行为）。
fn retry_window(attempt: u8) -> Duration {
    let exponent = u32::from(attempt.saturating_sub(1)).min(16);
    let window_ms = 6_u64.saturating_mul(1 << exponent).min(24);
    let half = window_ms / 2;
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos() as u64 ^ u64::from(attempt));
    Duration::from_millis(half + jitter % (window_ms - half + 1))
}

#[tokio::test]
#[ignore = "W12 负载夹具：显式运行，不在默认 test-db 门禁中执行"]
async fn perf_bid_hotspot_and_notify_commit() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let pool = store.postgres_pool().clone();
    seed(&pool).await.expect("写入合成历史");

    // 一个场次的全部 lot 作为热点集合；并发的每个事务都写同一个 auction 流序号。
    let auction_id: String =
        sqlx::query_scalar("SELECT id::text FROM public.auctions WHERE title = 'Perf auction 1'")
            .fetch_one(&pool)
            .await
            .expect("读取热点场次");
    let lot_ids: Vec<String> = sqlx::query_scalar(
        "SELECT l.id::text FROM public.lots l JOIN public.auctions a ON a.id = l.auction_id
         WHERE a.id = $1::uuid ORDER BY l.lot_number",
    )
    .bind(&auction_id)
    .fetch_all(&pool)
    .await
    .expect("读取热点 lot");
    assert_eq!(lot_ids.len(), usize::try_from(SEED_LOTS_PER_AUCTION).unwrap_or(20));
    let lot_ids = Arc::new(lot_ids);

    let concurrency = 16_usize;
    let per_task = 2_usize;
    let expected = concurrency * per_task;

    let before: Vec<i64> = sqlx::query_scalar(
        "SELECT state_version FROM public.lots WHERE auction_id = $1::uuid ORDER BY lot_number",
    )
    .bind(&auction_id)
    .fetch_all(&pool)
    .await
    .expect("读取 lot 版本基线");

    let fixed = run_contention(
        &pool,
        &auction_id,
        Arc::clone(&lot_ids),
        concurrency,
        per_task,
        false,
        NotifyMode::None,
    )
    .await
    .expect("无退避并发场景");
    let jittered = run_contention(
        &pool,
        &auction_id,
        Arc::clone(&lot_ids),
        concurrency,
        per_task,
        true,
        NotifyMode::None,
    )
    .await
    .expect("抖动退避并发场景");
    // 同一并发与退避下，比较三种唤醒位置的提交耗时——
    // 事务内 pg_notify（旧触发器）、提交后单独发送、以及不发送的对照。
    let notify_baseline =
        run_contention(&pool, &auction_id, Arc::clone(&lot_ids), 8, 2, true, NotifyMode::None)
            .await
            .expect("不含 pg_notify 的对照场景");
    let notified = run_contention(
        &pool,
        &auction_id,
        Arc::clone(&lot_ids),
        8,
        2,
        true,
        NotifyMode::InTransaction,
    )
    .await
    .expect("含事务内 pg_notify 的并发场景");
    let after_commit_notify = run_contention(
        &pool,
        &auction_id,
        Arc::clone(&lot_ids),
        8,
        2,
        true,
        NotifyMode::AfterCommit,
    )
    .await
    .expect("提交后 pg_notify 的并发场景");
    // 对照：16 个并发事务各写**不同** lot，因此任何序列化冲突只能来自共享的 stream 行。
    // 同一形状下比较“提交内序号”（旧路径）与“提交后补号”（新路径）。
    let cross_lot_legacy = run_contention(
        &pool,
        &auction_id,
        Arc::clone(&lot_ids),
        concurrency,
        1,
        true,
        NotifyMode::None,
    )
    .await
    .expect("不同 lot 的提交内序号对照");
    let deferred =
        run_contention_deferred(&pool, &auction_id, Arc::clone(&lot_ids), concurrency, 1)
            .await
            .expect("提交后补号的并发场景");

    let after: Vec<i64> = sqlx::query_scalar(
        "SELECT state_version FROM public.lots WHERE auction_id = $1::uuid ORDER BY lot_number",
    )
    .bind(&auction_id)
    .fetch_all(&pool)
    .await
    .expect("读取 lot 版本终点");

    // `attempts` 只记录成功提交所耗尝试次数，因此求和即成功事务数。
    let committed = |stats: &LoadStats| -> usize { stats.attempts.values().copied().sum() };
    let expected_notified = 16_usize;
    for (label, stats, expected_here) in [
        ("无退避", &fixed, expected),
        ("抖动退避", &jittered, expected),
        ("通知对照", &notify_baseline, expected_notified),
        ("含 pg_notify", &notified, expected_notified),
        ("提交后通知", &after_commit_notify, expected_notified),
        ("不同 lot 旧序号", &cross_lot_legacy, concurrency),
        ("提交后补号", &deferred, concurrency),
    ] {
        assert_eq!(
            committed(stats) + stats.exhausted + stats.other_failures,
            expected_here,
            "{label} 场景的每个事务都必须落在成功/耗尽/其它失败之一"
        );
    }

    let applied: i64 = before.iter().zip(after.iter()).map(|(start, end)| end - start).sum();
    assert_eq!(
        applied,
        i64::try_from(
            committed(&fixed)
                + committed(&jittered)
                + committed(&notify_baseline)
                + committed(&notified)
                + committed(&after_commit_notify)
                + committed(&cross_lot_legacy)
                + committed(&deferred)
        )
        .unwrap_or(0),
        "每个成功事务必须恰好应用一次，耗尽或其它失败不得留下部分写入"
    );

    let notify_commit_max = notified.commit_micros.iter().copied().max().unwrap_or(0);
    println!(
        "热点并发（{} 并发 × {} 事务/任务，同一场次同一 stream row）：\n           固定等待：成功 {}、耗尽 {}、p50 {}us、p95 {}us\n           抖动退避：成功 {}、耗尽 {}、p50 {}us、p95 {}us\n           含 pg_notify：成功 {}、单次提交上限 {}us\n  \
         通知对照：提交 p95 {}us；事务内通知提交 p95 {}us、p50 {}us；提交后通知提交 p95 {}us、p50 {}us、发送 p95 {}us\n           不同 lot 旧序号：成功 {}、冲突 {}、耗尽 {}、p50 {}us、p95 {}us\n           提交后补号：成功 {}、冲突 {}、耗尽 {}、p50 {}us、p95 {}us",
        concurrency,
        per_task,
        committed(&fixed),
        fixed.exhausted,
        fixed.percentile_micros(0.50),
        fixed.percentile_micros(0.95),
        committed(&jittered),
        jittered.exhausted,
        jittered.percentile_micros(0.50),
        jittered.percentile_micros(0.95),
        committed(&notified),
        notify_commit_max,
        notify_baseline.commit_percentile_micros(0.95),
        notified.commit_percentile_micros(0.95),
        notified.commit_percentile_micros(0.50),
        after_commit_notify.commit_percentile_micros(0.95),
        after_commit_notify.commit_percentile_micros(0.50),
        after_commit_notify.post_commit_notify_percentile_micros(0.95),
        committed(&cross_lot_legacy),
        cross_lot_legacy.conflicts,
        cross_lot_legacy.exhausted,
        cross_lot_legacy.percentile_micros(0.50),
        cross_lot_legacy.percentile_micros(0.95),
        committed(&deferred),
        deferred.conflicts,
        deferred.exhausted,
        deferred.percentile_micros(0.50),
        deferred.percentile_micros(0.95)
    );
    // 验收口径：提交后补号不允许出现冲突或耗尽，而且每个成功事务只应用一次。
    assert_eq!(deferred.conflicts, 0, "不同 lot 的提交后补号不得再产生序列化冲突");
    assert_eq!(deferred.exhausted, 0, "不同 lot 的提交后补号不得耗尽重试预算");
    assert_eq!(committed(&deferred), concurrency, "不同 lot 的提交后补号应全部提交成功");

    let report = json!({
        "schema": "auction.bid_hotspot_load.v1",
        "seeded_lots": SEED_AUCTIONS * SEED_LOTS_PER_AUCTION,
        "concurrency": concurrency,
        "transactions_per_task": per_task,
        "attempts_budget": 3,
        "fixed_backoff": fixed.to_json(),
        "jittered_backoff": jittered.to_json(),
        "notify_control": notify_baseline.to_json(),
        "with_pg_notify": notified.to_json(),
        "pg_notify_after_commit": after_commit_notify.to_json(),
        "cross_lot_legacy_sequence": cross_lot_legacy.to_json(),
        "deferred_sequence": deferred.to_json(),
        "applied_state_versions": applied,
    });
    let path = write_report("bid-hotspot-load", &report);
    println!("热点负载报告：{}", path.display());
    pool.close().await;
}

#[tokio::test]
#[ignore = "W12 负载夹具：显式运行，不在默认 test-db 门禁中执行"]
async fn perf_anonymous_read_burst_does_not_starve_bids() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = Arc::new(database.store().await.expect("连接测试数据库"));
    let pool = store.postgres_pool().clone();
    seed(&pool).await.expect("写入合成历史");

    let auction_id: String =
        sqlx::query_scalar("SELECT id::text FROM public.auctions WHERE title = 'Perf auction 2'")
            .fetch_one(&pool)
            .await
            .expect("读取热点场次");
    let lot_ids: Vec<String> = sqlx::query_scalar(
        "SELECT l.id::text FROM public.lots l JOIN public.auctions a ON a.id = l.auction_id
         WHERE a.id = $1::uuid ORDER BY l.lot_number",
    )
    .bind(&auction_id)
    .fetch_all(&pool)
    .await
    .expect("读取热点 lot");
    let lot_ids = Arc::new(lot_ids);

    // 两档匿名读突发共用同一物理池（`DatabaseStore` 的旧式语义），因此这组测量回答的是
    // "读抢连接会不会拖慢出价"，而不是另一套读池的效果。生产 Public 另有独立读限流/并发门，
    // 这里用"限流前/限流后"两档对比量化那道门的作用。
    let bids_alone =
        run_contention(&pool, &auction_id, Arc::clone(&lot_ids), 16, 1, true, NotifyMode::None)
            .await
            .expect("无读突发的出价对照");
    let (unlimited_reads, bids_with_unlimited) =
        timed_read_burst(&store, &pool, &auction_id, Arc::clone(&lot_ids), 24, 2)
            .await
            .expect("无读门时的混合负载");
    let (gated_reads, bids_with_gated) =
        timed_read_burst(&store, &pool, &auction_id, Arc::clone(&lot_ids), 4, 6)
            .await
            .expect("受限读门时的混合负载");

    for (label, stats) in [("无读门", &bids_with_unlimited), ("受限读门", &bids_with_gated)]
    {
        assert!(
            stats.attempts.values().copied().sum::<usize>() > 0,
            "{label} 场景应至少有一次成功的出价事务"
        );
    }
    for (label, reads) in [("无读门", &unlimited_reads), ("受限读门", &gated_reads)] {
        assert_eq!(reads.failures, 0, "{label} 场景的匿名读不应失败");
    }

    let solo_p95 = bids_alone.percentile_micros(0.95).max(1);
    let unlimited_ratio = bids_with_unlimited.percentile_micros(0.95) as f64 / solo_p95 as f64;
    let gated_ratio = bids_with_gated.percentile_micros(0.95) as f64 / solo_p95 as f64;
    println!(
        "混合负载（同一物理池）：出价 p95 对照 {}us → 无读门 {}x（{}, 读 p95 {}us）→          受限读门 {}x（{}, 读 p95 {}us）",
        solo_p95,
        unlimited_ratio,
        bids_with_unlimited.percentile_micros(0.95),
        unlimited_reads.p95_micros(),
        gated_ratio,
        bids_with_gated.percentile_micros(0.95),
        gated_reads.p95_micros()
    );
    // 受限读门必须把写出价延迟拉回对照量级；否则说明并发门值本身需要调小。
    assert!(gated_ratio <= 2.0, "受限读门下的出价 p95 应在对照的两倍内：{gated_ratio:.2}x");

    let report = json!({
        "schema": "auction.read_burst_load.v1",
        "seeded_lots": SEED_AUCTIONS * SEED_LOTS_PER_AUCTION,
        "bids_alone": bids_alone.to_json(),
        "read_burst_unlimited": {
            "concurrent_reads": 24,
            "requests": 48,
            "read_p95_us": unlimited_reads.p95_micros(),
            "read_failures": unlimited_reads.failures,
            "bid_p95_ratio_to_control": unlimited_ratio,
            "bids": bids_with_unlimited.to_json(),
        },
        "read_burst_gated": {
            "concurrent_reads": 4,
            "requests": 24,
            "read_p95_us": gated_reads.p95_micros(),
            "read_failures": gated_reads.failures,
            "bid_p95_ratio_to_control": gated_ratio,
            "bids": bids_with_gated.to_json(),
        },
    });
    let path = write_report("read-burst-load", &report);
    println!("混合负载报告：{}", path.display());
    pool.close().await;
}

/// 直接测量：投递入队事务在不同唤醒位置下的提交耗时。
///
/// 同一行插入（`deliveries` pending）在 000093 前会触发 `notify_delivery_ready` 在事务内
/// `pg_notify`；迁移后触发器删除，提交后单独发送。这里在测试库上临时重建与 000059 完全
/// 相同的触发器以测量“旧路径”，再删除它以测量“新路径”。只写合成投递，不领取、不发送。
async fn run_delivery_wakeup_load(
    pool: &PgPool,
    concurrency: usize,
    per_task: usize,
    mode: NotifyMode,
) -> Result<LoadStats, String> {
    let mut handles = Vec::with_capacity(concurrency);
    for task in 0..concurrency {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            let mut stats = LoadStats::default();
            for step in 0..per_task {
                let sequence = task * per_task + step;
                stats.transactions += 1;
                let delivery_id = Uuid::new_v5(
                    &Uuid::NAMESPACE_OID,
                    format!("perf-delivery-{mode:?}-{sequence}").as_bytes(),
                );
                let mut digest = [0_u8; 32];
                digest[..8].copy_from_slice(&(sequence as u64).to_le_bytes());
                let started = Instant::now();
                let mut transaction = match pool.begin().await {
                    Ok(transaction) => transaction,
                    Err(_) => {
                        stats.other_failures += 1;
                        break;
                    }
                };
                let inserted = sqlx::query(
                    "INSERT INTO public.deliveries
                        (id, command_id, request_digest, origin_kind, origin_id,
                         recipient, payload, status)
                     VALUES ($1, $2, $3, 'lot_watch', $4, $5, '{}'::jsonb, 'pending')",
                )
                .bind(delivery_id)
                .bind(Uuid::new_v5(
                    &Uuid::NAMESPACE_OID,
                    format!("perf-delivery-command-{mode:?}-{sequence}").as_bytes(),
                ))
                .bind(digest.as_slice())
                .bind(format!("perf-{sequence}"))
                .bind("perf@example.invalid")
                .execute(&mut *transaction)
                .await;
                if inserted.is_err() {
                    stats.other_failures += 1;
                    let _ = transaction.rollback().await;
                    continue;
                }
                let commit_started = Instant::now();
                match transaction.commit().await {
                    Ok(()) => {
                        stats.commit_micros.push(
                            u64::try_from(commit_started.elapsed().as_micros()).unwrap_or(u64::MAX),
                        );
                        if mode == NotifyMode::AfterCommit {
                            let notify_started = Instant::now();
                            let _ = sqlx::query("SELECT public.signal_delivery_ready()")
                                .execute(&pool)
                                .await;
                            stats.post_commit_notify_micros.push(
                                u64::try_from(notify_started.elapsed().as_micros())
                                    .unwrap_or(u64::MAX),
                            );
                        }
                        stats
                            .latencies_micros
                            .push(u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX));
                        *stats.attempts.entry(1).or_default() += 1;
                    }
                    Err(_) => stats.other_failures += 1,
                }
            }
            stats
        }));
    }

    let mut merged = LoadStats::default();
    for handle in handles {
        merged.merge(handle.await.map_err(|error| format!("负载任务失败：{error}"))?);
    }
    Ok(merged)
}

#[tokio::test]
#[ignore = "负载夹具：显式运行，不在默认 test-db 门禁中执行"]
async fn perf_delivery_wakeup_commit_path() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let pool = store.postgres_pool().clone();
    let concurrency = 16_usize;
    let per_task = 25_usize;

    // 重建 000059 的触发器，测量旧路径“事务内 NOTIFY”的提交耗时。
    sqlx::query(
        "CREATE FUNCTION public.perf_notify_delivery_ready() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN
             PERFORM pg_notify('auction_delivery_ready', '');
             RETURN NEW;
         END;
         $$",
    )
    .execute(&pool)
    .await
    .expect("建立测量用唤醒触发器函数");
    sqlx::query(
        "CREATE TRIGGER perf_delivery_ready_after_insert
         AFTER INSERT ON public.deliveries
         FOR EACH ROW WHEN (NEW.origin_kind = 'lot_watch' AND NEW.status = 'pending')
         EXECUTE FUNCTION public.perf_notify_delivery_ready()",
    )
    .execute(&pool)
    .await
    .expect("建立测量用唤醒触发器");

    let trigger_path =
        run_delivery_wakeup_load(&pool, concurrency, per_task, NotifyMode::InTransaction)
            .await
            .expect("旧触发器路径");

    sqlx::query("DROP TRIGGER perf_delivery_ready_after_insert ON public.deliveries")
        .execute(&pool)
        .await
        .expect("删除测量用触发器");
    sqlx::query("DROP FUNCTION public.perf_notify_delivery_ready()")
        .execute(&pool)
        .await
        .expect("删除测量用触发器函数");

    let after_commit =
        run_delivery_wakeup_load(&pool, concurrency, per_task, NotifyMode::AfterCommit)
            .await
            .expect("提交后唤醒路径");

    let expected = concurrency * per_task;
    assert_eq!(
        trigger_path.attempts.values().copied().sum::<usize>(),
        expected,
        "旧触发器路径不得出现失败"
    );
    assert_eq!(
        after_commit.attempts.values().copied().sum::<usize>(),
        expected,
        "提交后唤醒路径不得出现失败"
    );
    assert_eq!(trigger_path.other_failures, 0);
    assert_eq!(after_commit.other_failures, 0);
    println!(
        "投递唤醒（{} 并发 × {} 行/任务）：\n           事务内触发器提交 p95 {}us、p50 {}us；提交后发送提交 p95 {}us、p50 {}us、发送 p95 {}us",
        concurrency,
        per_task,
        trigger_path.commit_percentile_micros(0.95),
        trigger_path.commit_percentile_micros(0.50),
        after_commit.commit_percentile_micros(0.95),
        after_commit.commit_percentile_micros(0.50),
        after_commit.post_commit_notify_percentile_micros(0.95)
    );

    let report = json!({
        "schema": "auction.delivery_wakeup_commit.v1",
        "concurrency": concurrency,
        "rows_per_task": per_task,
        "trigger_in_transaction": trigger_path.to_json(),
        "notify_after_commit": after_commit.to_json(),
    });
    let path = write_report("delivery-wakeup-commit", &report);
    println!("投递唤醒报告：{}", path.display());
    pool.close().await;
}
