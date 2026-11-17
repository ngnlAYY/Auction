//! Public 竞价日志里的代理对代理竞争（运行账号）。
//!
//! 代理竞争只给胜者追加一条出价事实；被排位击败（`ranked_below`）的授权随这条事实公开出价人
//! 与上限，否则会出现同一人连续抬价、价格无故跳升的假象。快慢模式一视同仁：正在等待反击的
//! 慢速代理、当场落败的新建慢速代理，以及作为胜者当场出价的慢速代理都要覆盖。被普通出价超过
//! 上限而耗尽的代理不公开上限。落败授权不是出价事实，不改变总数与游标。
//!
//! 夹具只用 SQL 写入场次、装备、lot 与用户；竞价全部走生产端口 `DatabaseCompetitionCommands`。
//! 逐行落败查找经 `auction_storage::bid_log_benchmark_sql` 读生产查询原文做 EXPLAIN，确认它能走
//! 迁移 `000109` 的部分索引，而不是对只追加的转换表逐行顺序扫描。
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use auction_application::auction::{
    CompetitionCommandPort, CreateProxyCommand, PlaceBidCommand, PublicBidLogCursor,
    PublicBidLogEntry, PublicBidLogListRequest, PublicBidLogReader, PublicDefeatedProxy,
};
use auction_domain::{BidAmount, CommandId, LotId, ProxyMode, UserId};
use auction_storage::{DatabaseCompetitionCommands, DatabaseStore};
use auction_time::{Duration, Timestamp};
use serde_json::Value;
use sqlx::{AssertSqlSafe, PgPool, Row};
use uuid::Uuid;

use crate::support;
use crate::support::fixtures::{policy_snapshot, seed_user, stable_id};

const SEED: &str = "bid-log-proxies";
const START_PRICE: i64 = 50_000;
/// 0 ppm 时冻结策略的统一最低加价。
const BID_STEP: i64 = 30_000;
const PLAYERS: [&str; 5] = ["slow-a", "ordinary-b", "fast-c", "slow-e", "fast-f"];

struct Seeded {
    auction_id: Uuid,
    lot_id: Uuid,
    players: [Uuid; 5],
}

/// 写入一场 live 场次与一件 lot；只写夹具，不经过被测端口。
async fn seed(admin: &DatabaseStore) -> Seeded {
    let pool = admin.postgres_pool();
    let deadline = Timestamp::now() + Duration::hours(2);
    let opened = deadline - Duration::hours(4);
    let auction_id = stable_id(SEED, "auction");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, 'main', 1, 'bid log proxies', 'live', $2, $2, $3, $3, $2, $2, $4)",
    )
    .bind(auction_id)
    .bind(opened)
    .bind(deadline)
    .bind(policy_snapshot(deadline, START_PRICE, BID_STEP))
    .execute(pool)
    .await
    .expect("写入测试场次");
    sqlx::query("INSERT INTO public.realtime_streams (stream_key, next_sequence) VALUES ($1, 1)")
        .bind(format!("auction:{auction_id}"))
        .execute(pool)
        .await
        .expect("写入测试 stream");
    let players = PLAYERS.map(|player| stable_id(SEED, player));
    for (player, user_id) in PLAYERS.iter().zip(players) {
        seed_user(pool, user_id, &player_name(player)).await;
    }
    let equipment_id = stable_id(SEED, "equipment");
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, attributes, collected_at)
         VALUES ($1, 'main', '9200000', 'bid log proxies staff', 'Staff', '{}'::jsonb, $2)",
    )
    .bind(equipment_id)
    .bind(opened)
    .execute(pool)
    .await
    .expect("写入测试装备");
    let lot_id = stable_id(SEED, "lot");
    sqlx::query(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
             effective_end_at, category_key, category_number)
         VALUES ($1, $2, $3, 1, 'live', $4, $4, $5, 'Staff', 1)",
    )
    .bind(lot_id)
    .bind(auction_id)
    .bind(equipment_id)
    .bind(START_PRICE)
    .bind(deadline)
    .execute(pool)
    .await
    .expect("写入测试 lot");
    Seeded { auction_id, lot_id, players }
}

fn player_name(player: &str) -> String {
    format!("{SEED}-{player}")
}

fn create_proxy(
    step: &str,
    lot_id: Uuid,
    bidder_id: Uuid,
    maximum: i64,
    mode: ProxyMode,
) -> CreateProxyCommand {
    let lot_id = LotId::from_uuid(lot_id);
    let bidder_id = UserId::from_uuid(bidder_id);
    let maximum_amount = BidAmount::new(maximum).expect("合法代理上限");
    CreateProxyCommand {
        command_id: CommandId::from_uuid(stable_id(SEED, step)),
        lot_id,
        bidder_id,
        maximum_amount,
        mode,
        request_hash: CreateProxyCommand::legacy_request_hash(
            lot_id,
            bidder_id,
            mode,
            maximum_amount,
        )
        .expect("稳定请求摘要"),
        request_id: Some(format!("{SEED}-{step}")),
    }
}

fn place_bid(step: &str, lot_id: Uuid, bidder_id: Uuid, amount: i64) -> PlaceBidCommand {
    let lot_id = LotId::from_uuid(lot_id);
    let bidder_id = UserId::from_uuid(bidder_id);
    let amount = BidAmount::new(amount).expect("合法竞价金额");
    PlaceBidCommand {
        command_id: CommandId::from_uuid(stable_id(SEED, step)),
        lot_id,
        bidder_id,
        amount,
        request_hash: PlaceBidCommand::legacy_request_hash(lot_id, bidder_id, amount)
            .expect("稳定请求摘要"),
        request_id: Some(format!("{SEED}-{step}")),
    }
}

fn request(auction_id: Uuid, lot_id: Option<Uuid>) -> PublicBidLogListRequest {
    PublicBidLogListRequest {
        auction_id,
        world_id: None,
        lot_id,
        lot_code: None,
        limit: 20,
        offset: 0,
        cursor: None,
    }
}

fn defeated(player: &str, maximum_amount: i64) -> Option<PublicDefeatedProxy> {
    Some(PublicDefeatedProxy { bidder_name: player_name(player), maximum_amount })
}

/// 日志一行的出价人、金额、机制与随附的落败代理。
type Shape = (String, i64, String, Option<PublicDefeatedProxy>);

fn row(player: &str, amount: i64, mechanism: &str, defeated: Option<PublicDefeatedProxy>) -> Shape {
    (player_name(player), amount, mechanism.to_owned(), defeated)
}

/// 按日志降序取每行的出价人、金额、机制与随附的落败代理。
fn shape(entries: &[PublicBidLogEntry]) -> Vec<Shape> {
    entries
        .iter()
        .map(|entry| {
            (
                entry.bidder_name.clone(),
                entry.amount,
                entry.mechanism.clone(),
                entry.defeated_proxy.clone(),
            )
        })
        .collect()
}

/// 用生产查询原文取第一页的 JSON 计划；禁用顺序扫描只为暴露可用索引，不改变查询文本。
async fn first_page_plan(pool: &PgPool, auction_id: Uuid) -> String {
    let sql = auction_storage::bid_log_benchmark_sql::page_query_sql();
    let script = format!(
        "BEGIN;
         SET LOCAL enable_seqscan = off;
         PREPARE bid_log_probe AS {sql};
         EXPLAIN (FORMAT JSON) EXECUTE bid_log_probe(
             '{auction_id}', NULL, NULL, NULL, false, NULL, NULL, NULL, NULL, NULL, 20, 0
         );
         DEALLOCATE bid_log_probe;
         COMMIT;"
    );
    let rows = sqlx::raw_sql(AssertSqlSafe(script)).fetch_all(pool).await.expect("EXPLAIN");
    rows.iter()
        .find_map(|row| row.try_get::<sqlx::types::Json<Value>, _>(0).ok())
        .map(|plan| plan.0.to_string())
        .expect("EXPLAIN 返回 JSON 计划")
}

#[tokio::test]
async fn postgres_bid_log_shows_proxies_defeated_by_proxies_including_slow_ones() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let service = database.store().await.expect("以运行账号连接");
    let Seeded { auction_id, lot_id, players } = seed(&admin).await;
    let [slow_a, ordinary_b, fast_c, slow_e, fast_f] = players;
    let commands = DatabaseCompetitionCommands::new(Arc::new(service.clone()));

    // A 的慢速代理以起拍价领先；B 普通出价后 A 进入反击等待，尚未出价。
    commands
        .create_proxy(create_proxy("a-1", lot_id, slow_a, 200_000, ProxyMode::Slow))
        .await
        .expect("A 慢速代理");
    commands.place_bid(place_bid("b-1", lot_id, ordinary_b, 80_000)).await.expect("B 普通出价");
    // C 新建快速代理，与正在等待反击的 A 当场比上限：A 落败，C 以 A 的上限加一口出价。
    commands
        .create_proxy(create_proxy("c-1", lot_id, fast_c, 500_000, ProxyMode::Fast))
        .await
        .expect("C 快速代理");
    // A 以更高上限重建慢速代理，当场输给领先的 C：C 连续两条事实，正是线上被误读的形态。
    commands
        .create_proxy(create_proxy("a-2", lot_id, slow_a, 400_000, ProxyMode::Slow))
        .await
        .expect("A 重建慢速代理");
    // B 的普通出价超过 C 的上限：被普通出价击败的代理不公开上限。
    commands.place_bid(place_bid("b-2", lot_id, ordinary_b, 600_000)).await.expect("B 超过 C");
    // E 的慢速代理领先；F 的新代理输给它，慢速代理当场出价而不等待反击间隔。
    commands
        .create_proxy(create_proxy("e-1", lot_id, slow_e, 900_000, ProxyMode::Slow))
        .await
        .expect("E 慢速代理");
    commands
        .create_proxy(create_proxy("f-1", lot_id, fast_f, 700_000, ProxyMode::Fast))
        .await
        .expect("F 快速代理");

    let expected = [
        row("slow-e", 730_000, "proxy", defeated("fast-f", 700_000)),
        row("slow-e", 630_000, "proxy", None),
        row("ordinary-b", 600_000, "ordinary", None),
        row("fast-c", 430_000, "proxy", defeated("slow-a", 400_000)),
        row("fast-c", 230_000, "proxy", defeated("slow-a", 200_000)),
        row("ordinary-b", 80_000, "ordinary", None),
        row("slow-a", 50_000, "proxy", None),
    ];
    for filter in [None, Some(lot_id)] {
        let page = service
            .list_public_bid_log(request(auction_id, filter))
            .await
            .expect("读取竞价日志")
            .expect("场次可见");
        assert_eq!(shape(&page.entries), expected, "lot 过滤 {filter:?}");
        assert_eq!(page.total, Some(7), "落败授权不是出价事实，不计入总数");
        assert!(
            page.entries
                .iter()
                .filter(|entry| entry.defeated_proxy.is_some())
                .all(|entry| { entry.fact_index == 1 && entry.accepted_state_version > 0 }),
            "落败代理只随新建代理命令的第 1 条事实返回"
        );
    }

    // 编号页的中段与 cursor 页逐页读取都带同样的落败代理，且不重复、不丢行。
    let middle = service
        .list_public_bid_log(PublicBidLogListRequest {
            limit: 2,
            offset: 3,
            ..request(auction_id, None)
        })
        .await
        .expect("读取编号页")
        .expect("场次可见");
    assert_eq!(shape(&middle.entries), expected[3..5]);
    let mut cursor = Some(PublicBidLogCursor::Start);
    let mut walked = Vec::new();
    while let Some(next) = cursor.take() {
        let page = service
            .list_public_bid_log(PublicBidLogListRequest {
                limit: 3,
                cursor: Some(next),
                ..request(auction_id, None)
            })
            .await
            .expect("读取 cursor 页")
            .expect("场次可见");
        assert_eq!(page.total, None, "cursor 页不统计总数");
        walked.extend(shape(&page.entries));
        cursor = page.next_cursor;
    }
    assert_eq!(walked, expected);

    // 匿名路由的页大小不设上限：逐行落败查找必须是索引探测，代价才只随页大小增长。
    let plan = first_page_plan(service.postgres_pool(), auction_id).await;
    assert!(
        plan.contains("bid_proxy_transitions_ranked_below_origin_idx"),
        "落败查找没有走部分索引：{plan}"
    );

    service.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}
