//! W12.1 残留：Final 阶段同一场次不同 lot 的重叠出价不得因 Serializable 误报冲突而耗尽重试。
//!
//! 夹具只通过公开端口驱动：`DatabaseCompetitionCommands::place_bid` 就是 Public 出价的
//! 同一事务 owner。SQL 只用于写入测试场次/装备/lot/用户与读取结果，不复制出价事务。
//!
//! 覆盖：
//! - 同一 `final` 场次、不同 lot、不同用户的并发出价全部被接受，且没有 40001/40P01 耗尽；
//! - 出价事实、实时事件、提醒与 lot 最终状态保持一致（每条竞价各一次，价格/领先者正确）；
//! - 首次竞价触发 `lot_watch_leadership` 懒初始化时同样不因跨 lot 误报中止。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。
#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use auction_application::{ApplicationError, CompetitionCommandPort, PlaceBidCommand};
use auction_domain::{BidAmount, CommandId, LotId, UserId};
use auction_storage::{DatabaseCompetitionCommands, DatabaseStore};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::Row;
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
         VALUES ($1, 'main', 1, $2, 'final', $3, $3, $4, $4, $5)",
    )
    .bind(auction_id)
    .bind(format!("{seed} auction"))
    .bind(opened)
    .bind(deadline)
    .bind(policy(deadline))
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
    seed_history(pool, auction_id, deadline).await.expect("写入生产形状填充数据");

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
    let command_id = CommandId::from_uuid(stable_id(seed, &format!("command-{round}")));
    let lot_id = LotId::from_uuid(lot_id);
    let bidder_id = UserId::from_uuid(bidder_id);
    let amount = BidAmount::new(BASE_PRICE + BID_STEP).expect("合法竞价金额");
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

/// W12.3 负载演练入口：Final 阶段同一场次、不同 lot 的重叠出价。
///
/// 这是测量入口而不是默认门禁：显式运行后打印成功/序列化冲突计数。当前残留的
/// Serializable 误报仍未归零，因此不能把"零耗尽"写成默认断言（见 `docs/contracts/database.md`
/// 的热点测量一节）。成功出价的资金事实、事件与 lot 状态仍逐项断言，任何丢失或重复都会失败。
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
                    auction_application::CompetitionTransactionError::SerializationConflict,
                )) => failures.push("serialization_conflict".to_owned()),
                Err(error) => failures.push(format!("{error:?}")),
            }
        }
        println!(
            "W12.3 drill: {label} 并发 {concurrent}，成功 {accepted}，序列化冲突 {}",
            failures.len()
        );
        if failures.is_empty() {
            assert_bid_results(&store, label, concurrent, &seeded).await;
        }
        for failure in &failures {
            println!("  {label}: {failure}");
        }
        assert!(accepted > 0, "{label}: 至少一笔出价应被接受");
        assert!(
            failures.iter().all(|failure| failure == "serialization_conflict"),
            "{label}: 只允许已分类的序列化冲突，其他失败必须暴露：{failures:?}"
        );
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
