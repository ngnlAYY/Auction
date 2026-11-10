//! Final 阶段同一场次不同 lot 的重叠出价不得因 Serializable 误报冲突而耗尽重试。
//!
//! 夹具只通过公开端口驱动：`DatabaseCompetitionCommands::place_bid` 就是 Public 出价的
//! 同一事务 owner。SQL 只用于写入测试场次/装备/lot/用户与读取结果，不复制出价事务。
//!
//! 覆盖：
//! - 同一 `final` 场次、不同 lot、不同用户的并发出价全部被接受，且没有 40001/40P01 耗尽；
//! - 出价事实、实时事件、提醒与 lot 最终状态保持一致（每条竞价各一次，价格/领先者正确）；
//! - 提醒与自动标记来自提交后的 `lot_watch_events` 处理，不在 Serializable 出价事务内写入；
//! - 首次竞价触发 `lot_watch_leadership` 懒初始化时同样不因跨 lot 误报中止。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。
#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use auction_application::auction::{CompetitionCommandPort, PlaceBidCommand};
use auction_application::core::ApplicationError;
use auction_application::delivery::{DeliveryClaimExclusions, DeliveryWorkStore};
use auction_application::lot_watch_reminders::LotWatchReminderStore;
use auction_domain::{BidAmount, CommandId, LotId, UserId};
use auction_storage::{BidQueueMode, DatabaseCompetitionCommands, DatabaseStore};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::Row;
use std::time::Duration as StdDuration;
use tokio::sync::Barrier;
use uuid::Uuid;

use crate::support;

/// 并发出价数量默认值；受测试夹具 16 连接池限制，环境变量只允许 2..=12。
const DEFAULT_CONCURRENT_BIDS: usize = 8;
const BASE_PRICE: i64 = 120_000;
const BID_STEP: i64 = 1_000;

struct SeededRound {
    lots: Vec<Uuid>,
    bidders: Vec<Uuid>,
    leaders: Vec<Uuid>,
}

fn env_i64(name: &str, fallback: i64) -> i64 {
    std::env::var(name).ok().and_then(|value| value.parse().ok()).unwrap_or(fallback)
}

fn stable_id(seed: &str, suffix: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{seed}-{suffix}").as_bytes())
}

fn policy(deadline: DateTime<Utc>) -> serde_json::Value {
    json!({
        "schema_version": 5,
        "bidding_version": 2,
        "start_price": 50_000,
        "bid_increment_ppm": 0,
        "minimum_increment": BID_STEP,
        "soft_close_window_seconds": 900,
        "soft_close_extension_seconds": 900,
        "proxy_enabled": true,
        "proxy_default_mode": "fast",
        "proxy_slow_interval_seconds": 600,
        "proxy_max_outbid_uses": 0,
        "final_phase_lead_seconds": 900,
        "scheduled_end_at": deadline,
        "settlement": {
            "version": 1,
            "mode": "hold_curve_v1",
            "compensation_pool_enabled": true,
            "anchors": []
        },
        "recovery": {"version": 1, "mode": "remind_v1"}
    })
}

async fn seed_user(pool: &sqlx::PgPool, user_id: Uuid, username: &str) {
    let mut public_key_hash = [0_u8; 32];
    public_key_hash[..16].copy_from_slice(stable_id(username, "key").as_bytes());
    public_key_hash[16..].copy_from_slice(stable_id(username, "salt").as_bytes());
    sqlx::query("INSERT INTO public.users (id, username, public_key_hash) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(username)
        .bind(public_key_hash.as_slice())
        .execute(pool)
        .await
        .expect("写入测试用户");
}

/// 建立一场 `final` 场次：每个 lot 有不同领先者（并被标记关注）与不同竞价用户。
///
/// 领先周期行由迁移 000090 的 lot 触发器随 lot 创建事务一起写入，竞价不再懒初始化。
async fn seed_round(store: &DatabaseStore, seed: &str, lots: usize) -> SeededRound {
    seed_round_with_filler(store, seed, lots, 1, true).await
}

/// 同一形状的场次，可选场次序号与是否写入生产量级填充数据；多轮演练只在首轮填充。
async fn seed_round_with_filler(
    store: &DatabaseStore,
    seed: &str,
    lots: usize,
    sequence: i32,
    with_filler: bool,
) -> SeededRound {
    let pool = store.postgres_pool();
    let deadline = Utc::now() + Duration::hours(2);
    let opened = deadline - Duration::hours(4);
    let auction_id = stable_id(seed, "auction");

    let mut leaders = Vec::with_capacity(lots);
    let mut bidders = Vec::with_capacity(lots);
    for index in 0..lots {
        let leader = stable_id(seed, &format!("leader-{index}"));
        let bidder = stable_id(seed, &format!("bidder-{index}"));
        seed_user(pool, leader, &format!("{seed}-leader-{index}")).await;
        seed_user(pool, bidder, &format!("{seed}-bidder-{index}")).await;
        leaders.push(leader);
        bidders.push(bidder);
    }

    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, policy_snapshot)
         VALUES ($1, 'main', $6, $2, 'final', $3, $3, $4, $4, $5)",
    )
    .bind(auction_id)
    .bind(format!("{seed} auction"))
    .bind(opened)
    .bind(deadline)
    .bind(policy(deadline))
    .bind(sequence)
    .execute(pool)
    .await
    .expect("写入测试场次");

    // 场次创建事务会写入 `auction.created` 并建立序号计数行；竞价事件外键依赖它。
    sqlx::query("INSERT INTO public.realtime_streams (stream_key, next_sequence) VALUES ($1, 1)")
        .bind(format!("auction:{auction_id}"))
        .execute(pool)
        .await
        .expect("写入测试 stream");

    let mut seeded_lots = Vec::with_capacity(lots);
    for (index, leader) in leaders.iter().copied().enumerate().take(lots) {
        let equipment_id = stable_id(seed, &format!("equipment-{index}"));
        let lot_id = stable_id(seed, &format!("lot-{index}"));
        sqlx::query(
            "INSERT INTO public.equipment
                (id, world_id, external_equipment_id, name, category, collected_at)
             VALUES ($1, 'main', $2, $3, 'weapon', $4)",
        )
        .bind(equipment_id)
        .bind(format!("{seed}-eid-{index}"))
        .bind(format!("{seed} equipment {index}"))
        .bind(opened)
        .execute(pool)
        .await
        .expect("写入测试装备");

        sqlx::query(
            "INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
                 leader_user_id, effective_end_at, category_key, category_number)
             VALUES ($1, $2, $3, $4, 'live', 50000, $5, $6, $7, 'weapon', $4)",
        )
        .bind(lot_id)
        .bind(auction_id)
        .bind(equipment_id)
        .bind(i32::try_from(index + 1).expect("lot number"))
        .bind(BASE_PRICE)
        .bind(leader)
        .bind(deadline)
        .execute(pool)
        .await
        .expect("写入测试 lot");

        // 领先者关注 lot：被超越时走提醒入队与审计，覆盖 Final 阶段最重的写入形状。
        sqlx::query(
            "INSERT INTO public.lot_watches (user_id, lot_id, watched, channel, version)
             VALUES ($1, $2, true, 'mm', 1)",
        )
        .bind(leader)
        .bind(lot_id)
        .execute(pool)
        .await
        .expect("写入测试标记");

        seeded_lots.push(lot_id);
    }

    // 生产形状的填充数据：让热路径的选择走索引/位图而不是 8 行表的顺序扫描。
    // 顺序扫描会在 Serializable 下取得关系级谓词锁，把"不同 lot"误判为冲突，测出的
    // 数字不能代表真实负载。
    if with_filler {
        seed_history(pool, auction_id, deadline).await.expect("写入生产形状填充数据");
    }

    SeededRound { lots: seeded_lots, bidders, leaders }
}

fn filler_lots() -> i64 {
    env_i64("AUCTION_CROSSLOT_FILLER_LOTS", 800)
}
fn filler_commands() -> i64 {
    env_i64("AUCTION_CROSSLOT_FILLER_COMMANDS", 6_000)
}
fn filler_audits() -> i64 {
    env_i64("AUCTION_CROSSLOT_FILLER_AUDITS", 30_000)
}
fn filler_users() -> i64 {
    env_i64("AUCTION_CROSSLOT_FILLER_USERS", 200)
}

/// 填充历史 lot、命令、竞价与审计，使 `lots`/`auction_commands`/`bids`/`audit_records`
/// 达到真实量级；填充行不参与本轮竞价，也不进入提醒。
async fn seed_history(
    pool: &sqlx::PgPool,
    auction_id: Uuid,
    deadline: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO public.users (id, username, public_key_hash)
         SELECT gen_random_uuid(), 'crosslot-filler-user-' || g,
                decode(md5('crosslot-filler' || g) || md5('crosslot-salt' || g), 'hex')
         FROM generate_series(1, $1) g",
    )
    .bind(filler_users())
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, collected_at)
         SELECT gen_random_uuid(), 'main', 'crosslot-filler-eid-' || g,
                'crosslot filler equipment ' || g, 'weapon', $2
         FROM generate_series(1, $1) g",
    )
    .bind(filler_lots())
    .bind(deadline)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
             leader_user_id, effective_end_at, category_key, category_number)
         SELECT gen_random_uuid(), $1, e.id, 1000 + g, 'sold', 50000, 120000,
                (SELECT id FROM public.users WHERE username = 'crosslot-filler-user-' || (1 + g % $4)),
                $2, 'weapon', 1000 + g
         FROM generate_series(1, $3) g
         JOIN public.equipment e ON e.external_equipment_id = 'crosslot-filler-eid-' || g",
    )
    .bind(auction_id)
    .bind(deadline)
    .bind(filler_lots())
    .bind(filler_users())
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code, result_payload)
         SELECT gen_random_uuid(), 'place_bid', $1,
                decode(md5('crosslot-cmd' || g) || md5('crosslot-cmd2' || g), 'hex'),
                'accepted', '{}'::jsonb
         FROM generate_series(1, $2) g",
    )
    .bind(auction_id)
    .bind(filler_commands())
    .execute(pool)
    .await?;
    sqlx::query(
        "WITH numbered AS (
            SELECT command_id, row_number() OVER (ORDER BY command_id) AS rn
            FROM public.auction_commands WHERE command_kind = 'place_bid'
         ), fill_lots AS (
            SELECT id, row_number() OVER (ORDER BY lot_number) AS rn
            FROM public.lots WHERE status = 'sold' AND auction_id = $1
         )
         INSERT INTO public.bids
            (id, command_id, fact_index, lot_id, bidder_user_id, amount, mechanism,
             effective_end_at, accepted_state_version)
         SELECT gen_random_uuid(), n.command_id, 1, l.id,
                (SELECT id FROM public.users WHERE username = 'crosslot-filler-user-' || (1 + n.rn % $4)),
                121000, 'ordinary', $2, 2
         FROM numbered n JOIN fill_lots l ON l.rn = 1 + (n.rn - 1) % $3
         WHERE n.rn <= $3",
    )
    .bind(auction_id)
    .bind(deadline)
    .bind(filler_lots())
    .bind(filler_users())
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO public.lot_watches (user_id, lot_id, watched, channel, version)
         SELECT users.id, lots.id, true, 'mm', 1
         FROM public.lots AS lots
         CROSS JOIN (
            SELECT id FROM public.users WHERE username LIKE 'crosslot-filler-user-%' LIMIT 8
         ) AS users
         WHERE lots.status = 'sold' AND lots.auction_id = $1
         ON CONFLICT DO NOTHING",
    )
    .bind(auction_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result, source)
         SELECT gen_random_uuid(), now() - (g || ' seconds')::interval, 'public_user', 'filler',
                'place_bid', 'lot', 'filler-' || g, 'success', 'public'
         FROM generate_series(1, $1) g",
    )
    .bind(filler_audits())
    .execute(pool)
    .await?;
    for table in [
        "public.lots",
        "public.auction_commands",
        "public.bids",
        "public.audit_records",
        "public.users",
        "public.equipment",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!("ANALYZE {table}"))).execute(pool).await?;
    }
    Ok(())
}

fn place_bid_command(seed: &str, round: usize, lot_id: Uuid, bidder_id: Uuid) -> PlaceBidCommand {
    place_bid_command_with_amount(seed, round, lot_id, bidder_id, BASE_PRICE + BID_STEP)
}

fn place_bid_command_with_amount(
    seed: &str,
    round: usize,
    lot_id: Uuid,
    bidder_id: Uuid,
    amount: i64,
) -> PlaceBidCommand {
    let command_id = CommandId::from_uuid(stable_id(seed, &format!("command-{round}")));
    let lot_id = LotId::from_uuid(lot_id);
    let bidder_id = UserId::from_uuid(bidder_id);
    let amount = BidAmount::new(amount).expect("合法竞价金额");
    let request_hash =
        PlaceBidCommand::legacy_request_hash(lot_id, bidder_id, amount).expect("稳定请求摘要");
    PlaceBidCommand {
        command_id,
        lot_id,
        bidder_id,
        amount,
        request_hash,
        request_id: Some(format!("{seed}-request-{round}")),
    }
}

async fn assert_bid_results(
    store: &DatabaseStore,
    label: &str,
    concurrent: usize,
    seeded: &SeededRound,
) {
    let pool = store.postgres_pool();
    for (index, lot_id) in seeded.lots.iter().enumerate() {
        let bids: i64 = sqlx::query_scalar("SELECT count(*) FROM public.bids WHERE lot_id = $1")
            .bind(lot_id)
            .fetch_one(pool)
            .await
            .expect("读取竞价事实");
        assert_eq!(bids, 1, "{label}: lot {index} 应恰有一条竞价事实");
        let events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM public.realtime_events WHERE public_payload ->> 'lot_id' = $1",
        )
        .bind(lot_id.to_string())
        .fetch_one(pool)
        .await
        .expect("读取实时事件");
        assert_eq!(events, 1, "{label}: lot {index} 应恰有一条实时事件");
        let row = sqlx::query(
            "SELECT current_price, leader_user_id, state_version FROM public.lots WHERE id = $1",
        )
        .bind(lot_id)
        .fetch_one(pool)
        .await
        .expect("读取 lot 最终状态");
        let current_price: i64 = row.get("current_price");
        let leader: Uuid = row.get("leader_user_id");
        let state_version: i64 = row.get("state_version");
        assert_eq!(current_price, BASE_PRICE + BID_STEP, "{label}: lot {index} 价格");
        assert_eq!(leader, seeded.bidders[index], "{label}: lot {index} 领先者");
        assert_eq!(state_version, 2, "{label}: lot {index} 状态版本只推进一次");
    }

    // 每个被超越的领先者恰好收到一条提醒：通知事实、投递与审计一一对应，不因并发碰撞丢失。
    let notifications: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.lot_watch_notifications")
            .fetch_one(pool)
            .await
            .expect("读取提醒事实");
    assert_eq!(notifications, i64::try_from(concurrent).expect("并发数"), "{label}: 提醒事实数");
    let deliveries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.deliveries WHERE origin_kind = 'lot_watch'",
    )
    .fetch_one(pool)
    .await
    .expect("读取提醒投递");
    assert_eq!(deliveries, i64::try_from(concurrent).expect("并发数"), "{label}: 提醒投递数");
    for (index, leader) in seeded.leaders.iter().enumerate() {
        let notified: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM public.lot_watch_notifications WHERE user_id = $1",
        )
        .bind(leader)
        .fetch_one(pool)
        .await
        .expect("读取提醒收件用户");
        assert_eq!(notified, 1, "{label}: 领先者 {index} 恰好收到一条提醒");
    }
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.audit_records WHERE action LIKE 'lot_watch.%'",
    )
    .fetch_one(pool)
    .await
    .expect("读取标记审计");
    assert!(
        audits >= i64::try_from(concurrent).expect("并发数"),
        "{label}: 标记审计不得丢失（实际 {audits}）"
    );
}

/// 负载演练入口：Final 阶段同一场次、不同 lot 的重叠出价。
///
/// 显式运行后打印成功/序列化冲突计数。迁移 `000092` 把标记与被超提醒移出出价 Serializable
/// 窗口；该夹具在隔离 PG18 上要求零耗尽，同时逐项断言成功出价的资金事实、事件、提醒与 lot
/// 状态，任何丢失或重复都会失败（见 `docs/contracts/database.md` 的热点测量一节）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "W12.3 负载演练：显式运行以测量跨 lot 序列化冲突"]
async fn postgres_final_cross_lot_bids_do_not_exhaust_serializable_retries() {
    let concurrent = std::env::var("AUCTION_CROSSLOT_LOTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (2..=12).contains(value))
        .unwrap_or(DEFAULT_CONCURRENT_BIDS);
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let commands = Arc::new(DatabaseCompetitionCommands::new(Arc::new(store.clone())));

    // 迁移 000090 之后领先周期行随 lot 创建事务一起产生；首笔出价不再懒初始化。
    {
        let label = "final";
        let seed = "crosslot-final".to_owned();
        let seeded = seed_round(&store, &seed, concurrent).await;
        let barrier = Arc::new(Barrier::new(concurrent));
        let mut handles = Vec::with_capacity(concurrent);
        for index in 0..concurrent {
            let commands = Arc::clone(&commands);
            let barrier = Arc::clone(&barrier);
            let command =
                place_bid_command(&seed, index, seeded.lots[index], seeded.bidders[index]);
            handles.push(tokio::spawn(async move {
                // 让同一轮出价尽量落在同一瞬间，复现 Final 阶段的瞬时重叠。
                barrier.wait().await;
                commands.place_bid(command).await
            }));
        }

        let mut accepted = 0_usize;
        let mut failures = Vec::new();
        for handle in handles {
            match handle.await.expect("出价任务不得 panic") {
                Ok(_) => accepted += 1,
                Err(ApplicationError::CompetitionTransaction(
                    auction_application::core::CompetitionTransactionError::SerializationConflict,
                )) => failures.push("serialization_conflict".to_owned()),
                Err(error) => failures.push(format!("{error:?}")),
            }
        }
        println!(
            "W12.3 drill: {label} 并发 {concurrent}，成功 {accepted}，序列化冲突 {}",
            failures.len()
        );
        for failure in &failures {
            println!("  {label}: {failure}");
        }
        assert!(accepted > 0, "{label}: 至少一笔出价应被接受");
        assert!(failures.is_empty(), "{label}: 000092 后跨 lot 并发出价不得耗尽重试：{failures:?}");
        assert_bid_results(&store, label, concurrent, &seeded).await;
    }

    store.postgres_pool().close().await;
}

/// 命令账本的非锁定读取 + INSERT 仲裁：重复提交同一 `command_id` 只重放原结果，
/// 不写第二条竞价事实、事件或审计，也不再次推进 lot 状态版本。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_duplicate_bid_command_replays_without_second_fact() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let commands = DatabaseCompetitionCommands::new(Arc::new(store.clone()));
    let seeded = seed_round(&store, "duplicate", 2).await;
    let command = place_bid_command("duplicate", 0, seeded.lots[0], seeded.bidders[0]);
    let command_uuid = command.command_id.as_uuid();

    let first = commands.place_bid(command.clone()).await.expect("首次出价必须被接受");
    let replayed = commands.place_bid(command).await.expect("同 ID 同请求必须重放原结果");
    assert_eq!(
        first.plan.final_lot.state_version, replayed.plan.final_lot.state_version,
        "重放不得推进状态版本"
    );

    let pool = store.postgres_pool();
    let bids: i64 = sqlx::query_scalar("SELECT count(*) FROM public.bids WHERE lot_id = $1")
        .bind(seeded.lots[0])
        .fetch_one(pool)
        .await
        .expect("读取竞价事实");
    assert_eq!(bids, 1, "同一命令只能写一条竞价事实");
    let commands_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.auction_commands WHERE command_id = $1")
            .bind(command_uuid)
            .fetch_one(pool)
            .await
            .expect("读取命令账本");
    assert_eq!(commands_count, 1, "同一命令只能有一条账本记录");
    let counter: i32 = sqlx::query_scalar(
        "SELECT last_bid_fact_index FROM public.auction_commands WHERE command_id = $1",
    )
    .bind(command_uuid)
    .fetch_one(pool)
    .await
    .expect("读取事实序号计数");
    assert_eq!(counter, 1, "触发器必须把最后事实序号推进到 1");

    store.postgres_pool().close().await;
}

/// 同一 `command_id` 的**并发**重复提交：账本唯一主键仲裁，落在 lot 行锁后面的那一笔以 40001
/// 中止并在重试时命中账本、按原结果重放。只写一条事实/事件，lot 状态只推进一次——这是
/// 不变量 1「同 ID 同请求返回原结果」在并发形状下的直接回归（此前只有顺序重放用例）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_concurrent_duplicate_bid_command_replays_without_second_fact() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let commands = Arc::new(DatabaseCompetitionCommands::new(Arc::new(store.clone())));
    let seeded = seed_round(&store, "concurrent-duplicate", 1).await;
    let command = place_bid_command("concurrent-duplicate", 0, seeded.lots[0], seeded.bidders[0]);
    let command_uuid = command.command_id.as_uuid();

    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::with_capacity(2);
    for _ in 0..2 {
        let commands = Arc::clone(&commands);
        let barrier = Arc::clone(&barrier);
        let command = command.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            commands.place_bid(command).await
        }));
    }
    let mut versions = Vec::with_capacity(2);
    for handle in handles {
        let outcome =
            handle.await.expect("出价任务不得 panic").expect("并发同 ID 提交必须都返回原结果");
        versions.push(outcome.plan.final_lot.state_version);
    }
    assert_eq!(versions[0], versions[1], "并发同 ID 重放必须返回同一状态版本");

    let pool = store.postgres_pool();
    let bids: i64 = sqlx::query_scalar("SELECT count(*) FROM public.bids WHERE lot_id = $1")
        .bind(seeded.lots[0])
        .fetch_one(pool)
        .await
        .expect("读取竞价事实");
    assert_eq!(bids, 1, "并发同 ID 只能写一条竞价事实");
    let commands_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.auction_commands WHERE command_id = $1")
            .bind(command_uuid)
            .fetch_one(pool)
            .await
            .expect("读取命令账本");
    assert_eq!(commands_count, 1, "并发同 ID 只能有一条账本记录");
    let counter: i32 = sqlx::query_scalar(
        "SELECT last_bid_fact_index FROM public.auction_commands WHERE command_id = $1",
    )
    .bind(command_uuid)
    .fetch_one(pool)
    .await
    .expect("读取事实序号计数");
    assert_eq!(counter, 1, "并发重放不得再次推进事实序号");
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.realtime_events WHERE public_payload ->> 'lot_id' = $1",
    )
    .bind(seeded.lots[0].to_string())
    .fetch_one(pool)
    .await
    .expect("读取实时事件");
    assert_eq!(events, 1, "并发同 ID 只能写一条实时事件");
    let state_version: i64 =
        sqlx::query_scalar("SELECT state_version FROM public.lots WHERE id = $1")
            .bind(seeded.lots[0])
            .fetch_one(pool)
            .await
            .expect("读取 lot 状态版本");
    assert_eq!(state_version, 2, "并发重放不得二次推进 lot 状态版本");

    store.postgres_pool().close().await;
}

/// 迁移 000090：竞价事实连续性仍由数据库强制，且不再通过 `bids` 范围聚合读取。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_bid_fact_counter_rejects_non_contiguous_facts() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let commands = DatabaseCompetitionCommands::new(Arc::new(store.clone()));
    let seeded = seed_round(&store, "continuity", 1).await;

    let command = place_bid_command("continuity", 0, seeded.lots[0], seeded.bidders[0]);
    let command_id = command.command_id.as_uuid();
    commands.place_bid(command).await.expect("出价必须被接受");

    let pool = store.postgres_pool();
    // 直接写一条序号跳号的事实：触发器必须拒绝，而不是只依赖应用层。
    let skipped = sqlx::query(
        "INSERT INTO public.bids
            (id, command_id, fact_index, lot_id, bidder_user_id, amount, mechanism,
             effective_end_at, accepted_state_version)
         VALUES (gen_random_uuid(), $1, 3, $2, $3, 121000, 'ordinary', now(), 9)",
    )
    .bind(command_id)
    .bind(seeded.lots[0])
    .bind(seeded.bidders[0])
    .execute(pool)
    .await;
    let error = skipped.expect_err("不连续的竞价事实序号必须被拒绝");
    assert!(
        error.to_string().contains("23514") || error.to_string().contains("contiguous"),
        "拒绝必须来自事实序号约束：{error}"
    );

    store.postgres_pool().close().await;
}

/// 出价事务只追加标记事件，提醒在提交后的独立事务应用；
/// 重复处理不会产生第二条提醒，事件与提醒保持一一对应。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn postgres_lot_watch_outbox_round_trip_is_exactly_once() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let commands = Arc::new(DatabaseCompetitionCommands::new(Arc::new(store.clone())));
    let seeded = seed_round(&store, "outbox-once", 3).await;

    for index in 0..seeded.lots.len() {
        let command =
            place_bid_command("outbox-once", index, seeded.lots[index], seeded.bidders[index]);
        commands.place_bid(command).await.expect("出价必须被接受");
    }

    let pool = store.postgres_pool();
    let unfinished: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.lot_watch_events WHERE status <> 'done'")
            .fetch_one(pool)
            .await
            .expect("读取事件状态");
    assert_eq!(unfinished, 0, "提交后处理必须完成全部本轮事件");
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM public.lot_watch_events")
        .fetch_one(pool)
        .await
        .expect("读取事件数");
    assert_eq!(events, 3, "每个竞价事实至多一条事件");
    let notifications: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.lot_watch_notifications")
            .fetch_one(pool)
            .await
            .expect("读取提醒事实");
    assert_eq!(notifications, 3, "每个被超越的领先周期恰好一条提醒");

    let repeated =
        store.process_due("outbox-idempotent".to_owned(), 60, 16).await.expect("重复处理不得失败");
    assert_eq!(repeated.claimed, 0, "已完成事件不得再次领取");
    let notifications_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.lot_watch_notifications")
            .fetch_one(pool)
            .await
            .expect("读取提醒事实");
    assert_eq!(notifications_after, 3, "重复处理不得产生第二条提醒");

    store.postgres_pool().close().await;
}

/// 同一 lot 的竞价事件必须按 `state_version` 顺序应用：更早的事件未完成（含崩溃遗留的
/// 过期租约）时，后续事件不得越过顺序屏障。回收过期租约后按序补完，提醒不丢失、不重复。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_lot_watch_outbox_reclaims_lease_and_preserves_state_version_order() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let seeded = seed_round(&store, "outbox-order", 1).await;
    let pool = store.postgres_pool();
    let lot_id = seeded.lots[0];
    let first_bidder = seeded.bidders[0];
    let second_bidder = stable_id("outbox-order", "second-bidder");
    seed_user(pool, second_bidder, "outbox-order-second-bidder").await;
    let deadline: DateTime<Utc> =
        sqlx::query_scalar("SELECT effective_end_at FROM public.lots WHERE id = $1")
            .bind(lot_id)
            .fetch_one(pool)
            .await
            .expect("读取 lot 截止");

    let mut bids = Vec::new();
    for (index, (bidder, version)) in
        [(first_bidder, 2_i64), (second_bidder, 3_i64)].into_iter().enumerate()
    {
        let command_id = stable_id("outbox-order", &format!("command-{index}"));
        sqlx::query(
            "INSERT INTO public.auction_commands
                (command_id, command_kind, aggregate_id, request_hash, result_code)
             VALUES ($1, 'place_bid', $2, $3, 'accepted')",
        )
        .bind(command_id)
        .bind(stable_id("outbox-order", "auction"))
        .bind(vec![u8::try_from(index + 1).expect("fixture index"); 32])
        .execute(pool)
        .await
        .expect("写入测试命令");
        let bid_id = stable_id("outbox-order", &format!("bid-{index}"));
        sqlx::query(
            "INSERT INTO public.bids
                (id, command_id, fact_index, lot_id, bidder_user_id, amount, mechanism,
                 effective_end_at, accepted_state_version)
             VALUES ($1, $2, 1, $3, $4, $5, 'ordinary', $6, $7)",
        )
        .bind(bid_id)
        .bind(command_id)
        .bind(lot_id)
        .bind(bidder)
        .bind(BASE_PRICE + BID_STEP * (i64::try_from(index).expect("index") + 1))
        .bind(deadline)
        .bind(version)
        .execute(pool)
        .await
        .expect("写入测试竞价事实");
        // 生产事务会在同一事务内自动标记普通出价者；夹具补上同一事实。
        sqlx::query(
            "INSERT INTO public.lot_watches (user_id, lot_id, watched, channel, version)
             VALUES ($1, $2, true, 'mm', 1)
             ON CONFLICT (user_id, lot_id) DO UPDATE
             SET watched = true, version = lot_watches.version + 1,
                 updated_at = clock_timestamp()",
        )
        .bind(bidder)
        .bind(lot_id)
        .execute(pool)
        .await
        .expect("写入测试自动标记");
        bids.push((bid_id, bidder, version));
    }

    // 事件 2（state_version=2）延后可用，事件 3（state_version=3）已到期：顺序屏障必须让两者都不可领取。
    let second_event = stable_id("outbox-order", "event-2");
    let third_event = stable_id("outbox-order", "event-3");
    for (event_id, (bid_id, bidder, version), delay) in
        [(second_event, bids[0], Duration::hours(1)), (third_event, bids[1], Duration::zero())]
    {
        sqlx::query(
            "INSERT INTO public.lot_watch_events
                (event_id, event_kind, lot_id, user_id, bid_id, state_version,
                 occurred_at, next_attempt_at)
             VALUES ($1, 'bid_accepted', $2, $3, $4, $5, clock_timestamp(), clock_timestamp() + $6)",
        )
        .bind(event_id)
        .bind(lot_id)
        .bind(bidder)
        .bind(bid_id)
        .bind(version)
        .bind(delay)
        .execute(pool)
        .await
        .expect("写入测试事件");
    }
    let blocked =
        store.process_due("outbox-order".to_owned(), 60, 8).await.expect("顺序屏障查询不得失败");
    assert_eq!(blocked.claimed, 0, "更早未完成事件必须阻止后续事件");

    // 模拟崩溃遗留的 claim：事件 2 处于 processing 且租约过期，必须被回收后按序处理。
    sqlx::query(
        "UPDATE public.lot_watch_events
         SET status = 'processing', lease_owner = 'crashed-worker',
             lease_expires_at = clock_timestamp() - interval '1 second',
             next_attempt_at = clock_timestamp()
         WHERE event_id = $1",
    )
    .bind(second_event)
    .execute(pool)
    .await
    .expect("模拟过期租约");
    let first = store.process_due("outbox-order".to_owned(), 60, 8).await.expect("回收过期租约");
    assert_eq!(first.claimed, 2, "回收后按序处理同一 lot 的两条到期事件");
    assert_eq!(first.completed, 2);
    for event in [second_event, third_event] {
        let status: String =
            sqlx::query_scalar("SELECT status FROM public.lot_watch_events WHERE event_id = $1")
                .bind(event)
                .fetch_one(pool)
                .await
                .expect("读取事件状态");
        assert_eq!(status, "done");
    }

    let second = store.process_due("outbox-order".to_owned(), 60, 8).await.expect("重复处理");
    assert_eq!(second.claimed, 0, "已完成事件不得再次领取");
    assert_eq!(second.completed, 0);

    let leadership = sqlx::query(
        "SELECT leader_user_id, cycle FROM public.lot_watch_leadership WHERE lot_id = $1",
    )
    .bind(lot_id)
    .fetch_one(pool)
    .await
    .expect("读取领先周期");
    let leader: Uuid = leadership.get("leader_user_id");
    let cycle: i64 = leadership.get("cycle");
    assert_eq!(leader, second_bidder, "领先者必须来自最后一条已应用事实");
    assert_eq!(cycle, 3, "领先周期必须按事实状态版本推进");
    let notifications: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.lot_watch_notifications WHERE lot_id = $1")
            .bind(lot_id)
            .fetch_one(pool)
            .await
            .expect("读取提醒事实");
    assert_eq!(notifications, 2, "两个被超越的领先周期各一条提醒");
    let cycles: Vec<i64> = sqlx::query_scalar(
        "SELECT leadership_cycle FROM public.lot_watch_notifications WHERE lot_id = $1
         ORDER BY leadership_cycle",
    )
    .bind(lot_id)
    .fetch_all(pool)
    .await
    .expect("读取提醒周期");
    assert_eq!(cycles, vec![1, 2], "提醒周期必须来自被超越领先周期的事实状态版本");

    store.postgres_pool().close().await;
}

/// 普通出价的自动标记由 outbox 在提交后写入；事件到达后不得覆盖更新的显式取消。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_ordinary_bid_outbox_auto_watch_respects_newer_unwatch() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let commands = DatabaseCompetitionCommands::new(Arc::new(store.clone()));
    let seeded = seed_round(&store, "atomic-watch", 1).await;
    let lot_id = seeded.lots[0];
    let bidder = seeded.bidders[0];
    let command = place_bid_command("atomic-watch", 0, lot_id, bidder);
    commands.place_bid(command).await.expect("出价必须被接受");

    let pool = store.postgres_pool();
    let bid_id: Uuid = sqlx::query_scalar("SELECT id FROM public.bids WHERE lot_id = $1")
        .bind(lot_id)
        .fetch_one(pool)
        .await
        .expect("读取竞价事实");
    let delayed_event: Uuid =
        sqlx::query_scalar("SELECT event_id FROM public.lot_watch_events WHERE bid_id = $1")
            .bind(bid_id)
            .fetch_one(pool)
            .await
            .expect("读取竞价事件");
    for _ in 0..3 {
        store.process_due("atomic-watch".to_owned(), 60, 8).await.expect("处理竞价事件");
    }
    let event_status: String =
        sqlx::query_scalar("SELECT status FROM public.lot_watch_events WHERE event_id = $1")
            .bind(delayed_event)
            .fetch_one(pool)
            .await
            .expect("读取事件状态");
    assert_eq!(event_status, "done");
    let (watched, version): (bool, i64) = sqlx::query_as(
        "SELECT watched, version FROM public.lot_watches WHERE user_id = $1 AND lot_id = $2",
    )
    .bind(bidder)
    .bind(lot_id)
    .fetch_one(pool)
    .await
    .expect("事件完成后标记必须存在");
    assert!(watched, "普通出价的提交后事件必须自动标记");
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.audit_records
         WHERE action = 'lot_watch.bid_created' AND entity_id = $1",
    )
    .bind(lot_id.to_string())
    .fetch_one(pool)
    .await
    .expect("读取标记审计");
    assert_eq!(audits, 1, "自动标记只写一条审计");

    // 模拟用户在出价提交后显式取消，随后又到达一条延迟的竞价事件；它不能改回关注。
    sqlx::query(
        "UPDATE public.lot_watches SET watched = false, version = $3,
             updated_at = clock_timestamp()
         WHERE user_id = $1 AND lot_id = $2",
    )
    .bind(bidder)
    .bind(lot_id)
    .bind(version + 1)
    .execute(pool)
    .await
    .expect("写入更新的取消关注");
    // 模拟 Worker 重试一条已经完成后才到达的旧事件；处理必须幂等且不能恢复关注。
    sqlx::query(
        "UPDATE public.lot_watch_events
         SET status = 'pending', lease_owner = NULL, lease_expires_at = NULL,
             completed_at = NULL, next_attempt_at = clock_timestamp()
         WHERE event_id = $1",
    )
    .bind(delayed_event)
    .execute(pool)
    .await
    .expect("重新排入延迟竞价事件");
    for _ in 0..3 {
        store.process_due("atomic-watch".to_owned(), 60, 8).await.expect("处理延迟竞价事件");
    }
    let delayed_status: String =
        sqlx::query_scalar("SELECT status FROM public.lot_watch_events WHERE event_id = $1")
            .bind(delayed_event)
            .fetch_one(pool)
            .await
            .expect("读取延迟事件状态");
    assert_eq!(delayed_status, "done", "延迟事件必须真实处理完成");
    let watched: bool = sqlx::query_scalar(
        "SELECT watched FROM public.lot_watches WHERE user_id = $1 AND lot_id = $2",
    )
    .bind(bidder)
    .bind(lot_id)
    .fetch_one(pool)
    .await
    .expect("读取取消后的标记");
    assert!(!watched, "延迟事件不得覆盖更新的显式取消");
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.audit_records
         WHERE action = 'lot_watch.bid_created' AND entity_id = $1",
    )
    .bind(lot_id.to_string())
    .fetch_one(pool)
    .await
    .expect("读取栅栏后的标记审计");
    assert_eq!(audits, 1, "被栅栏拒绝的自动标记不得追加审计");

    store.postgres_pool().close().await;
}

/// 新建代理的自动标记由 outbox 完成；延迟到达的事件不能覆盖之后的显式取消。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_lot_watch_outbox_does_not_override_newer_proxy_unwatch() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let seeded = seed_round(&store, "outbox-proxy", 1).await;
    let pool = store.postgres_pool();
    let lot_id = seeded.lots[0];
    let user_id = seeded.bidders[0];
    let command_id = stable_id("outbox-proxy", "command");
    let authorization_id = stable_id("outbox-proxy", "authorization");
    let event_id = stable_id("outbox-proxy", "event");

    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'place_proxy', $2, $3, 'accepted')",
    )
    .bind(command_id)
    .bind(stable_id("outbox-proxy", "auction"))
    .bind([7_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入测试命令");
    sqlx::query(
        "INSERT INTO public.bid_proxy_authorizations
            (authorization_id, lot_id, user_id, authorization_sequence, usage_number,
             maximum_amount, mode, status, state_version, creation_command_id, created_at,
             updated_at)
         VALUES ($1, $2, $3, 1, 1, 500000, 'fast', 'active', 1, $4,
                 clock_timestamp(), clock_timestamp())",
    )
    .bind(authorization_id)
    .bind(lot_id)
    .bind(user_id)
    .bind(command_id)
    .execute(pool)
    .await
    .expect("写入测试代理");
    sqlx::query(
        "INSERT INTO public.lot_watch_events
            (event_id, event_kind, lot_id, user_id, authorization_id, state_version,
             occurred_at, next_attempt_at)
         VALUES ($1, 'proxy_created', $2, $3, $4, 1, clock_timestamp(), clock_timestamp())",
    )
    .bind(event_id)
    .bind(lot_id)
    .bind(user_id)
    .bind(authorization_id)
    .execute(pool)
    .await
    .expect("写入测试事件");
    // 模拟用户在授权提交后、事件被 Worker 处理前显式取消关注。
    sqlx::query(
        "INSERT INTO public.lot_watches (user_id, lot_id, watched, channel, version, updated_at)
         VALUES ($1, $2, false, 'mm', 2, clock_timestamp() + interval '1 second')",
    )
    .bind(user_id)
    .bind(lot_id)
    .execute(pool)
    .await
    .expect("写入更新的取消关注");

    let batch =
        store.process_due("outbox-proxy".to_owned(), 60, 8).await.expect("处理代理创建事件");
    assert_eq!(batch.claimed, 1);
    assert_eq!(batch.completed, 1);
    let watched: bool = sqlx::query_scalar(
        "SELECT watched FROM public.lot_watches WHERE user_id = $1 AND lot_id = $2",
    )
    .bind(user_id)
    .bind(lot_id)
    .fetch_one(pool)
    .await
    .expect("读取自动标记");
    assert!(!watched, "延迟事件不得覆盖更新的显式取消");
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.audit_records
         WHERE action = 'lot_watch.proxy_created' AND entity_id = $1",
    )
    .bind(lot_id.to_string())
    .fetch_one(pool)
    .await
    .expect("读取标记审计");
    assert_eq!(audits, 0, "事件处理不再补写自动标记审计");
    let reminders: i64 = sqlx::query_scalar("SELECT count(*) FROM public.lot_watch_notifications")
        .fetch_one(pool)
        .await
        .expect("读取提醒事实");
    assert_eq!(reminders, 0, "新建代理本身不产生被超提醒");

    store.postgres_pool().close().await;
}

/// 多轮演练按本轮 lot 断言：同一数据库里还会有其它轮次的事实，不能再用全局计数。
async fn assert_round_results(
    store: &DatabaseStore,
    label: &str,
    concurrent: usize,
    seeded: &SeededRound,
) {
    let pool = store.postgres_pool();
    for (index, lot_id) in seeded.lots.iter().enumerate() {
        let bids: i64 = sqlx::query_scalar("SELECT count(*) FROM public.bids WHERE lot_id = $1")
            .bind(lot_id)
            .fetch_one(pool)
            .await
            .expect("读取竞价事实");
        assert_eq!(bids, 1, "{label}: lot {index} 应恰有一条竞价事实");
        let row = sqlx::query(
            "SELECT current_price, leader_user_id, state_version FROM public.lots WHERE id = $1",
        )
        .bind(lot_id)
        .fetch_one(pool)
        .await
        .expect("读取 lot 最终状态");
        let current_price: i64 = row.get("current_price");
        let leader: Uuid = row.get("leader_user_id");
        let state_version: i64 = row.get("state_version");
        assert_eq!(current_price, BASE_PRICE + BID_STEP, "{label}: lot {index} 价格");
        assert_eq!(leader, seeded.bidders[index], "{label}: lot {index} 领先者");
        assert_eq!(state_version, 2, "{label}: lot {index} 状态版本只推进一次");
    }
    let notifications: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.lot_watch_notifications WHERE lot_id = ANY($1)",
    )
    .bind(&seeded.lots[..])
    .fetch_one(pool)
    .await
    .expect("读取本轮提醒事实");
    assert_eq!(notifications, i64::try_from(concurrent).expect("并发数"), "{label}: 提醒事实数");
}

/// 把演练场次移出活动集合，允许同一世界创建下一轮 `final` 场次。
async fn retire_auction(store: &DatabaseStore, seed: &str) {
    sqlx::query("UPDATE public.auctions SET status = 'settled' WHERE id = $1")
        .bind(stable_id(seed, "auction"))
        .execute(store.postgres_pool())
        .await
        .expect("回收演练场次");
}

/// 当前数据库里仍被持有的会话级 advisory lock 数；每个测试独占克隆库，因此只统计本夹具。
async fn advisory_lock_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_locks
         WHERE locktype = 'advisory'
           AND database = (SELECT oid FROM pg_database WHERE datname = current_database())",
    )
    .fetch_one(pool)
    .await
    .expect("读取 advisory lock 数")
}

/// 在预算内等待条件成立；超时返回 `false`，由调用方断言给出可读失败信息。
async fn wait_for_condition<F, Fut>(budget: StdDuration, mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + budget;
    loop {
        if check().await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(StdDuration::from_millis(10)).await;
    }
}

/// 两个模拟 Public 实例共用一个数据库：同一场次的跨 lot 出价必须经会话 advisory lock 串行。
///
/// 夹具用一条独立连接把 lot0 的行锁住，让实例一的出价停在事务内并持有队列凭证；此时实例二的
/// 出价（不同 lot、不同连接池、不同进程内互斥量）必须在**队列**上等待，不能直接进入事务。
/// 放行后两笔都成功，且没有 40001——这才是跨实例串行而不是仅进程内互斥的证据。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn postgres_bid_queue_serializes_cross_lot_across_instances() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let instance_one = database.store().await.expect("实例一连接");
    let instance_two = database.store().await.expect("实例二连接");
    let seeded = seed_round(&instance_one, "queue-cross-instance", 2).await;
    let commands_one = Arc::new(DatabaseCompetitionCommands::new(Arc::new(instance_one.clone())));
    let commands_two = Arc::new(DatabaseCompetitionCommands::new(Arc::new(instance_two.clone())));
    let pool = instance_one.postgres_pool();

    // 阻塞实例一出价的事务内进度：事务已经持有队列凭证，只是卡在 lot 行锁上。
    let mut blocker = pool.acquire().await.expect("取得阻塞连接");
    sqlx::query("BEGIN").execute(&mut *blocker).await.expect("开启阻塞事务");
    sqlx::query("SELECT id FROM public.lots WHERE id = $1 FOR UPDATE")
        .bind(seeded.lots[0])
        .execute(&mut *blocker)
        .await
        .expect("锁定 lot0");

    let first_command =
        place_bid_command("queue-cross-instance", 0, seeded.lots[0], seeded.bidders[0]);
    let first = {
        let commands = Arc::clone(&commands_one);
        tokio::spawn(async move { commands.place_bid(first_command).await })
    };
    assert!(
        wait_for_condition(StdDuration::from_secs(5), || async {
            advisory_lock_count(pool).await >= 1
        })
        .await,
        "实例一必须已经持有该场次的会话锁"
    );

    let second_command =
        place_bid_command("queue-cross-instance", 1, seeded.lots[1], seeded.bidders[1]);
    let mut second = {
        let commands = Arc::clone(&commands_two);
        tokio::spawn(async move { commands.place_bid(second_command).await })
    };
    // 不同进程内互斥量、不同连接池：只有跨实例会话锁能让它停下来。
    assert!(
        tokio::time::timeout(StdDuration::from_millis(300), &mut second).await.is_err(),
        "实例二在实例一持锁期间不得进入同一场次的事务"
    );

    sqlx::query("ROLLBACK").execute(&mut *blocker).await.expect("释放阻塞事务");
    // 阻塞连接必须先归还：`PgPool::close` 会等待所有借出连接结束。
    drop(blocker);
    first.await.expect("实例一出价任务不得 panic").expect("实例一出价必须成功");
    second.await.expect("实例二出价任务不得 panic").expect("实例二出价必须成功");

    let deadline = Instant::now() + StdDuration::from_secs(5);
    while advisory_lock_count(pool).await != 0 && Instant::now() < deadline {
        tokio::time::sleep(StdDuration::from_millis(10)).await;
    }
    assert_eq!(advisory_lock_count(pool).await, 0, "队列凭证必须在出价结束后释放");
    let bids: i64 = sqlx::query_scalar("SELECT count(*) FROM public.bids WHERE lot_id = ANY($1)")
        .bind(&seeded.lots[..2])
        .fetch_one(pool)
        .await
        .expect("读取竞价事实");
    assert_eq!(bids, 2, "跨实例串行后两笔出价各写一条事实");

    instance_one.postgres_pool().close().await;
    instance_two.postgres_pool().close().await;
}

/// 被拒绝的命令同样要归还队列：业务错误不得把同一场次永久锁死。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_bid_queue_releases_after_rejected_command() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let commands = DatabaseCompetitionCommands::new(Arc::new(store.clone()));
    let seeded = seed_round(&store, "queue-rejected", 1).await;
    let pool = store.postgres_pool();

    // 金额未超过当前价：事务内被领域规则拒绝，事务回滚，但队列凭证必须已经归还。
    let rejected = place_bid_command_with_amount(
        "queue-rejected",
        0,
        seeded.lots[0],
        seeded.bidders[0],
        BASE_PRICE,
    );
    let error = commands.place_bid(rejected).await.expect_err("未超过当前价的出价必须被拒绝");
    assert!(matches!(error, ApplicationError::Domain(_)), "拒绝必须来自领域规则：{error:?}");
    assert!(
        wait_for_condition(StdDuration::from_secs(5), || async {
            advisory_lock_count(pool).await == 0
        })
        .await,
        "被拒绝的命令必须释放会话锁"
    );

    let accepted = place_bid_command("queue-rejected", 1, seeded.lots[0], seeded.bidders[0]);
    commands.place_bid(accepted).await.expect("释放后同场次必须能正常出价");
    let bids: i64 = sqlx::query_scalar("SELECT count(*) FROM public.bids WHERE lot_id = $1")
        .bind(seeded.lots[0])
        .fetch_one(pool)
        .await
        .expect("读取竞价事实");
    assert_eq!(bids, 1, "被拒绝的命令不得留下竞价事实");

    store.postgres_pool().close().await;
}

/// 取消（客户端断连 / 请求被丢弃）必须归还队列凭证：凭证的 `Drop` 负责解锁会话锁。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn postgres_bid_queue_releases_when_command_is_canceled() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let commands = Arc::new(DatabaseCompetitionCommands::new(Arc::new(store.clone())));
    let seeded = seed_round(&store, "queue-canceled", 1).await;
    let pool = store.postgres_pool();

    let mut blocker = pool.acquire().await.expect("取得阻塞连接");
    sqlx::query("BEGIN").execute(&mut *blocker).await.expect("开启阻塞事务");
    sqlx::query("SELECT id FROM public.lots WHERE id = $1 FOR UPDATE")
        .bind(seeded.lots[0])
        .execute(&mut *blocker)
        .await
        .expect("锁定 lot0");

    let command = place_bid_command("queue-canceled", 0, seeded.lots[0], seeded.bidders[0]);
    let task = {
        let commands = Arc::clone(&commands);
        tokio::spawn(async move { commands.place_bid(command).await })
    };
    assert!(
        wait_for_condition(StdDuration::from_secs(5), || async {
            advisory_lock_count(pool).await >= 1
        })
        .await,
        "取消前必须确认命令已经持有队列凭证"
    );

    task.abort();
    assert!(task.await.expect_err("任务必须被取消").is_cancelled());
    sqlx::query("ROLLBACK").execute(&mut *blocker).await.expect("释放阻塞事务");
    drop(blocker);
    assert!(
        wait_for_condition(StdDuration::from_secs(5), || async {
            advisory_lock_count(pool).await == 0
        })
        .await,
        "取消后会话锁必须被释放或随会话关闭回收"
    );

    let retry = place_bid_command("queue-canceled", 1, seeded.lots[0], seeded.bidders[0]);
    commands.place_bid(retry).await.expect("取消后同一场次必须仍可出价");

    store.postgres_pool().close().await;
}

/// 六轮 Final 阶段负载演练：排队前后各测一次，输出分位延迟、失败分类与连接池峰值。
///
/// 显式运行：`cargo test -p auction-storage --features test-db --test postgres \
/// -- --ignored --nocapture postgres_bid_queue_drill`
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "W12.1 六轮负载演练：显式运行以测量排队前后的延迟与连接池占用"]
async fn postgres_bid_queue_drill_reports_latency_and_pool_usage() {
    const ROUNDS: usize = 6;
    let concurrent = std::env::var("AUCTION_CROSSLOT_LOTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (2..=12).contains(value))
        .unwrap_or(DEFAULT_CONCURRENT_BIDS);
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let mut filler_seeded = false;
    // 同一世界的场次序号唯一，两个模式共用递增序号。
    let mut auction_sequence = 0_i32;

    for mode in [BidQueueMode::Direct, BidQueueMode::Queued] {
        let store = database.store().await.expect("连接测试数据库").with_bid_queue_mode(mode);
        let commands = Arc::new(DatabaseCompetitionCommands::new(Arc::new(store.clone())));
        let pool = store.postgres_pool();
        let mut latencies = Vec::new();
        let mut conflicts = 0_usize;
        let mut timeouts = 0_usize;
        let mut other = 0_usize;
        let mut accepted_total = 0_usize;
        let peak = Arc::new(AtomicU64::new(0));

        for round in 0..ROUNDS {
            let seed = format!("drill-{mode:?}-{round}");
            auction_sequence += 1;
            let seeded =
                seed_round_with_filler(&store, &seed, concurrent, auction_sequence, !filler_seeded)
                    .await;
            filler_seeded = true;
            let barrier = Arc::new(Barrier::new(concurrent));
            let sampler = {
                let pool = pool.clone();
                let peak = Arc::clone(&peak);
                tokio::spawn(async move {
                    loop {
                        let tracked =
                            u32::try_from(pool.num_idle()).unwrap_or(u32::MAX).min(pool.size());
                        peak.fetch_max(u64::from(pool.size() - tracked), Ordering::Relaxed);
                        tokio::time::sleep(StdDuration::from_millis(2)).await;
                    }
                })
            };
            let mut handles = Vec::with_capacity(concurrent);
            for index in 0..concurrent {
                let commands = Arc::clone(&commands);
                let barrier = Arc::clone(&barrier);
                let command =
                    place_bid_command(&seed, index, seeded.lots[index], seeded.bidders[index]);
                handles.push(tokio::spawn(async move {
                    barrier.wait().await;
                    let started = Instant::now();
                    let result = commands.place_bid(command).await;
                    (started.elapsed(), result)
                }));
            }
            for handle in handles {
                let (elapsed, result) = handle.await.expect("出价任务不得 panic");
                latencies.push(elapsed);
                match result {
                    Ok(_) => accepted_total += 1,
                    Err(ApplicationError::CompetitionTransaction(
                        auction_application::core::CompetitionTransactionError::SerializationConflict,
                    )) => conflicts += 1,
                    Err(ApplicationError::CompetitionTransaction(
                        auction_application::core::CompetitionTransactionError::QueueWaitTimeout,
                    )) => timeouts += 1,
                    Err(error) => {
                        if other < 3 {
                            println!("  轮 {round} 其它失败：{error:?}");
                        }
                        other += 1;
                    }
                }
            }
            sampler.abort();
            // 对照组的 40001 会让部分 lot 没有事实，只对排队组断言完整结果。
            if mode == BidQueueMode::Queued {
                assert_round_results(&store, &seed, concurrent, &seeded).await;
            }
            // 每个世界同时只允许一个活动场次；本轮结束后把场次移出活动集合，
            // 下一轮（含另一个模式的首轮）才能在同一世界再建 `final` 场次。
            retire_auction(&store, &seed).await;
        }

        latencies.sort_unstable();
        let percentile = |fraction: f64| {
            if latencies.is_empty() {
                return 0.0;
            }
            let index = ((latencies.len() as f64 - 1.0) * fraction).round() as usize;
            latencies[index].as_secs_f64() * 1_000.0
        };
        println!(
            "W12.1 drill mode={mode:?} 轮数={ROUNDS} 并发={concurrent} 总数={} 成功={accepted_total} \
             序列化耗尽={conflicts} 排队超时={timeouts} 其它失败={other}",
            latencies.len()
        );
        println!(
            "  p50={:.1}ms p95={:.1}ms p99={:.1}ms 连接池峰值={} （池上限 {}）",
            percentile(0.50),
            percentile(0.95),
            percentile(0.99),
            peak.load(Ordering::Relaxed),
            pool.options().get_max_connections(),
        );
        if mode == BidQueueMode::Queued {
            assert_eq!(conflicts, 0, "排队后不得再出现序列化耗尽");
            assert_eq!(timeouts, 0, "排队预算内不得超时");
            assert_eq!(other, 0, "排队后不得出现其它失败");
        }
        pool.close().await;
    }
}

/// 事务内唤醒触发器已删除；投递入队事务提交后由处理进程单独发送唤醒，
/// 通知到达时投递必须已经提交且可领取。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_delivery_wakeup_is_published_after_commit() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let seeded = seed_round(&store, "wakeup-after-commit", 1).await;
    let pool = store.postgres_pool();

    let triggers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_trigger
         WHERE tgname = 'delivery_ready_after_insert' AND NOT tgisinternal",
    )
    .fetch_one(pool)
    .await
    .expect("读取投递触发器");
    assert_eq!(triggers, 0, "事务内唤醒触发器必须已删除");
    let functions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_proc WHERE proname = 'notify_delivery_ready'")
            .fetch_one(pool)
            .await
            .expect("读取旧唤醒函数");
    assert_eq!(functions, 0, "事务内唤醒函数必须已删除");

    let mut wakeup = store.subscribe_wakeups().await.expect("订阅投递唤醒");
    let commands = DatabaseCompetitionCommands::new(Arc::new(store.clone()));
    let command = place_bid_command("wakeup-after-commit", 0, seeded.lots[0], seeded.bidders[0]);
    commands.place_bid(command).await.expect("出价必须被接受");

    tokio::time::timeout(StdDuration::from_secs(10), wakeup.wait())
        .await
        .expect("提交后必须发送唤醒")
        .expect("唤醒订阅不得断开");

    // 通知本身不代替领取；到达后投递必须可被 claim 查询选中。
    let now = Utc::now();
    let claims = store
        .claim_due(
            "wakeup-after-commit".to_owned(),
            now,
            now + Duration::minutes(2),
            StdDuration::from_secs(60),
            DeliveryClaimExclusions::default(),
            1,
        )
        .await
        .expect("领取被超提醒投递");
    assert_eq!(claims.len(), 1, "唤醒到达后必须能领取已提交投递");

    // LISTEN 连接由订阅句柄独占；先释放，否则 close 会一直等待该连接归还。
    drop(wakeup);
    store.postgres_pool().close().await;
}

/// 崩溃窗口：投递事务提交后、通知发出前进程崩溃不会丢失投递；
/// 轮询扫描直接按持久状态领取，通知不是正确性的前提。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_delivery_wakeup_crash_before_notify_falls_back_to_scan() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let pool = store.postgres_pool();
    let seed = "wakeup-crash-window";

    // 模拟“提交成功但没有发送通知”：直接写入已提交的 pending 投递，全程不调用唤醒。
    // 用 `manual` 来源隔离 lot/watch 资格退役逻辑，只验证 claim 不依赖通知。
    let delivery_id = stable_id(seed, "delivery");
    let payload = json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "external_mail_id": "wakeup-crash",
        "template": "outbid",
        "result_code": null,
        "user_key_user_id": null,
        "credits": null,
        "cod": null,
        "attachment_ids": [],
        "body": "outbid notice",
    });
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status)
         VALUES ($1, $2, $3, 'manual', $4, $5, $6, 'pending')",
    )
    .bind(delivery_id)
    .bind(stable_id(seed, "command"))
    .bind([7_u8; 32].as_slice())
    .bind(seed)
    .bind("wakeup-recipient")
    .bind(&payload)
    .execute(pool)
    .await
    .expect("写入已提交投递");

    let now = Utc::now();
    let claims = store
        .claim_due(
            "wakeup-scan".to_owned(),
            now,
            now + Duration::minutes(2),
            StdDuration::from_secs(60),
            DeliveryClaimExclusions::default(),
            1,
        )
        .await
        .expect("轮询领取");
    assert_eq!(claims.len(), 1, "未发送通知时轮询仍必须领取已提交投递");
    assert_eq!(claims[0].delivery_id, delivery_id);

    store.postgres_pool().close().await;
}

/// 通知只是提示；同一批唤醒下并发领取不会重复处理同一条投递。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_delivery_wakeup_does_not_duplicate_claims() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let pool = store.postgres_pool();
    let seed = "wakeup-duplicate";

    let delivery_id = stable_id(seed, "delivery");
    let payload = json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "external_mail_id": "wakeup-duplicate",
        "template": "outbid",
        "result_code": null,
        "user_key_user_id": null,
        "credits": null,
        "cod": null,
        "attachment_ids": [],
        "body": "outbid notice",
    });
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status)
         VALUES ($1, $2, $3, 'manual', $4, $5, $6, 'pending')",
    )
    .bind(delivery_id)
    .bind(stable_id(seed, "command"))
    .bind([8_u8; 32].as_slice())
    .bind(seed)
    .bind("wakeup-recipient")
    .bind(&payload)
    .execute(pool)
    .await
    .expect("写入已提交投递");

    let now = Utc::now();
    let claim = |worker: &'static str| {
        store.claim_due(
            worker.to_owned(),
            now,
            now + Duration::minutes(2),
            StdDuration::from_secs(60),
            DeliveryClaimExclusions::default(),
            1,
        )
    };
    let (first, second) = tokio::join!(claim("wakeup-duplicate-a"), claim("wakeup-duplicate-b"));
    let first = first.expect("并发领取 A");
    let second = second.expect("并发领取 B");
    let claimed =
        first.iter().chain(second.iter()).filter(|c| c.delivery_id == delivery_id).count();
    assert_eq!(claimed, 1, "同一唤醒下的并发领取必须恰好成功一次");

    store.postgres_pool().close().await;
}
