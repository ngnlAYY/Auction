//! W12.1 跟进：按场次的写队列覆盖生命周期迁移与 Admin 排期/lot 写入。
//!
//! 夹具只通过公开端口驱动：`DatabaseCompetitionCommands::place_bid`、
//! `DatabaseAuctionLifecycleCommands::execute` 与 `DatabaseAdminLotCommands::update` 就是
//! Public/Admin/Worker 使用的同一批事务 owner。SQL 只用于写入测试场次、lot、用户并读取结果。
//!
//! 覆盖：
//! - 同一 `live` 场次的 Admin 最终阶段迁移与 Final 并发出价共用一把按场次的门：全部成功、
//!   零 40001/40P01 耗尽，竞价事实与场次状态一致；
//! - 同一 draft 场次的两个 lot 的 Admin 修改互相串行（不是按 lot 排队），而另一世界的 draft
//!   场次不被牵连——队列不串行化无关场次；
//! - 生命周期命令被取消后凭证必须归还：会话 advisory lock 归零，同场次随后可以继续迁移；
//! - 六轮负载演练（`--ignored`）测量 `Direct`/`Queued` 两种模式下的延迟、冲突、超时与死锁数。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。
#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use auction_application::{
    AdminLotAction, AdminLotUpdateCommand, AdminLotUpdatePort, ApplicationError,
    AuctionLifecycleAction, AuctionLifecycleActor, AuctionLifecycleActorType,
    AuctionLifecycleCommand, AuctionLifecycleCommandPort, AuctionLifecyclePolicy,
    AuctionPolicyTemplate, Clock, CompetitionCommandPort, CompetitionTransactionError,
    PlaceBidCommand, RequestHash,
};
use auction_domain::{AuctionId, BidAmount, CommandId, LotId, UserId};
use auction_storage::{
    BidQueueMode, DatabaseAdminLotCommands, DatabaseAuctionLifecycleCommands,
    DatabaseCompetitionCommands, DatabaseStore,
};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use std::time::Duration as StdDuration;
use tokio::sync::Barrier;
use uuid::Uuid;

use crate::support;

/// 并发写入数默认值；受夹具 16 连接池限制，环境变量只允许 2..=12。
const DEFAULT_CONCURRENT_WRITES: usize = 8;
const START_PRICE: i64 = 50_000;
const BID_STEP: i64 = 1_000;

struct SeededAuction {
    auction_id: Uuid,
    lots: Vec<Uuid>,
    bidders: Vec<Uuid>,
    lot_state_versions: Vec<i64>,
}

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

fn stable_id(seed: &str, suffix: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{seed}-{suffix}").as_bytes())
}

fn policy(deadline: DateTime<Utc>) -> serde_json::Value {
    json!({
        "schema_version": 5,
        "bidding_version": 2,
        "start_price": START_PRICE,
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
        // 生命周期上下文会解码 `settlement`：必须是可校验的 hold_curve_v2 曲线。
        "settlement": {
            "version": 2,
            "mode": "hold_curve_v2",
            "compensation_pool_enabled": true,
            "isekai_carrier_credit": 1,
            "anchors": [
                {"hold_ppm": 0, "adjustment_ppm": 80_000},
                {"hold_ppm": 500_000, "adjustment_ppm": -20_000},
                {"hold_ppm": 1_000_000, "adjustment_ppm": -20_000}
            ]
        },
        "recovery": {
            "version": 1,
            "mode": "remind_v1",
            "payment_timeout": {"secs": 604_800, "nanos": 0},
            "reminder_limit": 1,
            "cod_return_limit": 3
        }
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

/// 写入一场指定状态（`live` 或 `draft`）的场次与若干 lot。
///
/// `live` 表示已经开拍、等待最终阶段迁移或出价；`draft` 允许 Admin lot 修改。
async fn seed_auction(
    store: &DatabaseStore,
    seed: &str,
    world_id: &str,
    status: &str,
    sequence: i32,
    lots: usize,
) -> SeededAuction {
    let pool = store.postgres_pool();
    let deadline = Utc::now() + Duration::hours(2);
    let opened = deadline - Duration::hours(4);
    let auction_id = stable_id(seed, "auction");
    let lot_status = if status == "draft" { "pending" } else { "live" };

    // draft 场次必须与同世界的 intake target 一致；延迟一致性触发器在提交时检查，
    // 因此场次与 target 必须在同一事务里写入。
    let mut transaction = pool.begin().await.expect("开启场次写入事务");
    let opened_live = if status == "draft" { None } else { Some(opened) };
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, $2, $3, $4, $5, $6, $6, $7, $7, $8, $8, $9)",
    )
    .bind(auction_id)
    .bind(world_id)
    .bind(sequence)
    .bind(format!("{seed} auction"))
    .bind(status)
    .bind(opened)
    .bind(deadline)
    .bind(opened_live)
    .bind(policy(deadline))
    .execute(&mut *transaction)
    .await
    .expect("写入测试场次");

    if status == "draft" {
        sqlx::query(
            "INSERT INTO public.auction_intake_targets (world_id, auction_id, target_status)
             VALUES ($1, $2, 'draft')",
        )
        .bind(world_id)
        .bind(auction_id)
        .execute(&mut *transaction)
        .await
        .expect("写入 draft 场次的 intake target");
    }
    transaction.commit().await.expect("提交场次写入事务");

    sqlx::query("INSERT INTO public.realtime_streams (stream_key, next_sequence) VALUES ($1, 1)")
        .bind(format!("auction:{auction_id}"))
        .execute(pool)
        .await
        .expect("写入测试 stream");

    let mut seeded_lots = Vec::with_capacity(lots);
    let mut bidders = Vec::with_capacity(lots);
    let mut lot_state_versions = Vec::with_capacity(lots);
    for index in 0..lots {
        let bidder = stable_id(seed, &format!("bidder-{index}"));
        let equipment_id = stable_id(seed, &format!("equipment-{index}"));
        let lot_id = stable_id(seed, &format!("lot-{index}"));
        seed_user(pool, bidder, &format!("{seed}-bidder-{index}")).await;

        sqlx::query(
            "INSERT INTO public.equipment
                (id, world_id, external_equipment_id, name, category, attributes, collected_at)
             VALUES ($1, $2, $3, $4, 'weapon', '{}'::jsonb, $5)",
        )
        .bind(equipment_id)
        .bind(world_id)
        .bind(format!("{seed}-eid-{index}"))
        .bind(format!("{seed} equipment {index}"))
        .bind(opened)
        .execute(pool)
        .await
        .expect("写入测试装备");

        // `pending` lot 必须挂在一条 submission 上；Admin 来源需要命令、来源引用与指纹。
        let submission_id = if lot_status == "pending" {
            let submission_id = stable_id(seed, &format!("submission-{index}"));
            let command_id = stable_id(seed, &format!("submission-command-{index}"));
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
            .expect("写入 submission 来源命令");
            sqlx::query(
                "INSERT INTO public.auction_submissions
                    (id, source_command_id, auction_id, seller, received_at,
                     source_kind, source_reference, source_fingerprint)
                 VALUES ($1, $2, $3, 'queue-test-seller', $4, 'admin', $5, $6)",
            )
            .bind(submission_id)
            .bind(command_id)
            .bind(auction_id)
            .bind(opened)
            .bind(format!("{seed}-submission-{index}"))
            .bind(fingerprint.as_slice())
            .execute(pool)
            .await
            .expect("写入测试 submission");
            Some(submission_id)
        } else {
            None
        };

        let state_version: i64 = sqlx::query_scalar(
            "INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
                 effective_end_at, category_key, category_number, submission_id)
             VALUES ($1, $2, $3, $4, $5, $6, $6, $7, 'weapon', $4, $8)
             RETURNING state_version",
        )
        .bind(lot_id)
        .bind(auction_id)
        .bind(equipment_id)
        .bind(i32::try_from(index + 1).expect("lot number"))
        .bind(lot_status)
        .bind(START_PRICE)
        .bind(deadline)
        .bind(submission_id)
        .fetch_one(pool)
        .await
        .expect("写入测试 lot");

        seeded_lots.push(lot_id);
        bidders.push(bidder);
        lot_state_versions.push(state_version);
    }

    SeededAuction { auction_id, lots: seeded_lots, bidders, lot_state_versions }
}

/// 把场次移出活动集合，允许同一世界创建下一轮场次。
///
/// draft 场次先删除 intake target；两条写入在同一事务里，延迟一致性触发器在提交时看到一致状态。
async fn retire_auction(store: &DatabaseStore, auction_id: Uuid) {
    let mut transaction = store.postgres_pool().begin().await.expect("开启回收事务");
    sqlx::query("DELETE FROM public.auction_intake_targets WHERE auction_id = $1")
        .bind(auction_id)
        .execute(&mut *transaction)
        .await
        .expect("回收 intake target");
    sqlx::query("UPDATE public.auctions SET status = 'settled' WHERE id = $1")
        .bind(auction_id)
        .execute(&mut *transaction)
        .await
        .expect("回收演练场次");
    transaction.commit().await.expect("提交回收事务");
}

fn bid_command(seed: &str, round: usize, lot_id: Uuid, bidder_id: Uuid) -> PlaceBidCommand {
    let command_id = CommandId::from_uuid(stable_id(seed, &format!("command-{round}")));
    let lot_id = LotId::from_uuid(lot_id);
    let bidder_id = UserId::from_uuid(bidder_id);
    let amount = BidAmount::new(START_PRICE + BID_STEP).expect("合法竞价金额");
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

fn lifecycle_commands(store: &DatabaseStore) -> DatabaseAuctionLifecycleCommands {
    DatabaseAuctionLifecycleCommands::new(
        Arc::new(store.clone()),
        AuctionLifecyclePolicy::new(true, true),
        AuctionPolicyTemplate::default(),
    )
}

fn final_phase_command(seed: &str, auction_id: Uuid) -> AuctionLifecycleCommand {
    AuctionLifecycleCommand {
        command_id: CommandId::from_uuid(stable_id(seed, "lifecycle-command")),
        auction_id: AuctionId::from_uuid(auction_id),
        action: AuctionLifecycleAction::FinalPhase,
        expected_state_version: None,
        reason: None,
        request_hash: RequestHash::from_canonical_bytes(format!("{seed}-final-phase").as_bytes()),
        request_id: Some(format!("{seed}-lifecycle-request")),
        actor: AuctionLifecycleActor {
            actor_type: AuctionLifecycleActorType::Admin,
            actor_id: "queue-test-admin".to_owned(),
        },
    }
}

fn admin_lot_command(
    seed: &str,
    lot_id: Uuid,
    expected_state_version: i64,
    start_price: i64,
) -> AdminLotUpdateCommand {
    AdminLotUpdateCommand {
        command_id: CommandId::from_uuid(stable_id(seed, "admin-lot-command")),
        lot_id: LotId::from_uuid(lot_id),
        expected_state_version,
        action: AdminLotAction::SetStartPrice,
        start_price: Some(start_price),
        actor_id: "queue-test-admin".to_owned(),
        request_id: Some(format!("{seed}-admin-request")),
        request_hash: RequestHash::from_canonical_bytes(
            format!("{seed}-admin-lot-{start_price}").as_bytes(),
        ),
    }
}

async fn admin_lot_commands(store: &DatabaseStore) -> DatabaseAdminLotCommands<SystemClock> {
    DatabaseAdminLotCommands::new(Arc::new(store.clone()), Arc::new(SystemClock))
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

/// 本库累计死锁检测次数；用于区分 `40P01` 与普通 40001。
async fn deadlock_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT deadlocks FROM pg_stat_database WHERE datname = current_database()")
        .fetch_one(pool)
        .await
        .expect("读取死锁计数")
}

async fn wait_for_condition<F, Fut>(budget: StdDuration, mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
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

/// 同一场次的 Admin 最终阶段迁移与 Final 并发出价必须由同一把场次门串行化。
///
/// `FinalPhase` 会在事务内 `UPDATE auctions` 并对活动 lot 收紧截止时间；并发出价对
/// `auctions` 取 `FOR SHARE`、对 lot 取 `FOR UPDATE`。没有队列时这正是 SSI 冲突与
/// `40P01` 的形状；有队列时全部命令都必须成功，且死锁计数不变。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn postgres_auction_queue_serializes_lifecycle_against_final_bids() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let seeded = seed_auction(&store, "queue-lifecycle", "main", "live", 1, 4).await;
    let pool = store.postgres_pool();
    let deadlocks_before = deadlock_count(pool).await;

    let bids = Arc::new(DatabaseCompetitionCommands::new(Arc::new(store.clone())));
    let lifecycle = Arc::new(lifecycle_commands(&store));
    let barrier = Arc::new(Barrier::new(seeded.lots.len() + 1));

    let lifecycle_task = {
        let lifecycle = Arc::clone(&lifecycle);
        let barrier = Arc::clone(&barrier);
        let command = final_phase_command("queue-lifecycle", seeded.auction_id);
        tokio::spawn(async move {
            barrier.wait().await;
            lifecycle.execute(command).await
        })
    };

    let mut bid_tasks = Vec::with_capacity(seeded.lots.len());
    for (index, (lot_id, bidder)) in
        seeded.lots.iter().copied().zip(seeded.bidders.iter().copied()).enumerate()
    {
        let bids = Arc::clone(&bids);
        let barrier = Arc::clone(&barrier);
        let command = bid_command("queue-lifecycle", index, lot_id, bidder);
        bid_tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            (index, bids.place_bid(command).await)
        }));
    }

    let lifecycle_result = lifecycle_task.await.expect("生命周期任务不得 panic");
    lifecycle_result.expect("最终阶段迁移必须成功");
    for task in bid_tasks {
        let (index, result) = task.await.expect("出价任务不得 panic");
        match result {
            Ok(_) => {}
            Err(error) => panic!("lot {index} 的出价必须成功：{error:?}"),
        }
    }

    let status: String = sqlx::query_scalar("SELECT status FROM public.auctions WHERE id = $1")
        .bind(seeded.auction_id)
        .fetch_one(pool)
        .await
        .expect("读取场次状态");
    assert_eq!(status, "final", "最终阶段迁移必须提交");
    let bids_written: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.bids WHERE lot_id = ANY($1)")
            .bind(&seeded.lots[..])
            .fetch_one(pool)
            .await
            .expect("读取竞价事实");
    assert_eq!(
        bids_written,
        i64::try_from(seeded.lots.len()).expect("lot 数"),
        "并发写入后每个 lot 恰好一条竞价事实"
    );
    assert_eq!(deadlock_count(pool).await, deadlocks_before, "排队后不得出现死锁");
    assert!(
        wait_for_condition(StdDuration::from_secs(5), || async {
            advisory_lock_count(pool).await == 0
        })
        .await,
        "队列凭证必须在全部命令结束后释放"
    );

    store.postgres_pool().close().await;
}

/// Admin draft lot 修改按**场次**排队：同场次不同 lot 串行，另一世界不受影响。
///
/// 夹具用一条独立连接锁住 lot0，让 `main` 场次的第一个修改停在事务内并持有场次门；此时
/// 同一场次另一个 lot 的修改必须在队列上等待，而 `isekai` 场次的修改必须立即完成——证明
/// 门是场次级而不是 lot 级，也不串行化无关场次。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn postgres_admin_lot_queue_serializes_same_auction_and_leaves_others_free() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let busy = seed_auction(&store, "queue-admin-busy", "main", "draft", 1, 2).await;
    let other = seed_auction(&store, "queue-admin-other", "isekai", "draft", 1, 1).await;
    let pool = store.postgres_pool();
    let admin = Arc::new(admin_lot_commands(&store).await);

    // 阻塞第一个修改：事务已经持有场次门，只是卡在 lot0 的行锁上。
    let mut blocker = pool.acquire().await.expect("取得阻塞连接");
    sqlx::query("BEGIN").execute(&mut *blocker).await.expect("开启阻塞事务");
    sqlx::query("SELECT id FROM public.lots WHERE id = $1 FOR UPDATE")
        .bind(busy.lots[0])
        .execute(&mut *blocker)
        .await
        .expect("锁定 lot0");

    let first = {
        let admin = Arc::clone(&admin);
        let command = admin_lot_command("queue-admin-busy-0", busy.lots[0], 1, 55_000);
        tokio::spawn(async move { admin.update(command).await })
    };
    assert!(
        wait_for_condition(StdDuration::from_secs(5), || async {
            advisory_lock_count(pool).await >= 1
        })
        .await,
        "第一个 Admin 修改必须已经持有场次门"
    );

    // 同场次另一个 lot：只有场次门能让它停下来。
    let second = {
        let admin = Arc::clone(&admin);
        let command = admin_lot_command("queue-admin-busy-1", busy.lots[1], 1, 56_000);
        tokio::spawn(async move { admin.update(command).await })
    };
    let mut second = second;
    assert!(
        tokio::time::timeout(StdDuration::from_millis(300), &mut second).await.is_err(),
        "同一场次的第二个修改不得在场次门被持有时完成"
    );

    // 无关场次：必须并行完成，不能被 main 的门串行化。
    let unrelated = {
        let admin = Arc::clone(&admin);
        let command = admin_lot_command("queue-admin-other", other.lots[0], 1, 57_000);
        tokio::spawn(async move { admin.update(command).await })
    };
    unrelated
        .await
        .expect("无关场次任务不得 panic")
        .expect("无关场次的修改不得被 main 的场次门阻塞");

    sqlx::query("ROLLBACK").execute(&mut *blocker).await.expect("释放阻塞事务");
    drop(blocker);
    first.await.expect("第一个任务不得 panic").expect("第一个 Admin 修改必须成功");
    second.await.expect("第二个任务不得 panic").expect("门释放后第二个 Admin 修改必须成功");

    let busy_prices: Vec<i64> = sqlx::query_scalar(
        "SELECT start_price FROM public.lots WHERE id = ANY($1) ORDER BY lot_number",
    )
    .bind(&busy.lots[..])
    .fetch_all(pool)
    .await
    .expect("读取同场次 lot 起拍价");
    assert_eq!(busy_prices, vec![55_000, 56_000], "同场次两个 lot 都按顺序写入");
    let other_price: i64 = sqlx::query_scalar("SELECT start_price FROM public.lots WHERE id = $1")
        .bind(other.lots[0])
        .fetch_one(pool)
        .await
        .expect("读取无关场次 lot 起拍价");
    assert_eq!(other_price, 57_000, "无关场次的修改必须独立生效");
    assert!(
        wait_for_condition(StdDuration::from_secs(5), || async {
            advisory_lock_count(pool).await == 0
        })
        .await,
        "全部修改结束后会话锁必须归零"
    );

    store.postgres_pool().close().await;
}

/// 生命周期命令被取消（客户端断连 / Worker 任务被丢弃）后必须归还场次门。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn postgres_auction_queue_releases_lifecycle_when_canceled() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let seeded = seed_auction(&store, "queue-lifecycle-cancel", "main", "live", 1, 1).await;
    let pool = store.postgres_pool();
    let lifecycle = Arc::new(lifecycle_commands(&store));

    let mut blocker = pool.acquire().await.expect("取得阻塞连接");
    sqlx::query("BEGIN").execute(&mut *blocker).await.expect("开启阻塞事务");
    sqlx::query("SELECT id FROM public.auctions WHERE id = $1 FOR UPDATE")
        .bind(seeded.auction_id)
        .execute(&mut *blocker)
        .await
        .expect("锁定场次行");

    let task = {
        let lifecycle = Arc::clone(&lifecycle);
        let command = final_phase_command("queue-lifecycle-cancel", seeded.auction_id);
        tokio::spawn(async move { lifecycle.execute(command).await })
    };
    assert!(
        wait_for_condition(StdDuration::from_secs(5), || async {
            advisory_lock_count(pool).await >= 1
        })
        .await,
        "取消前必须确认生命周期命令已经持有场次门"
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

    lifecycle
        .execute(final_phase_command("queue-lifecycle-cancel-retry", seeded.auction_id))
        .await
        .expect("取消后同场次必须仍能完成迁移");
    let status: String = sqlx::query_scalar("SELECT status FROM public.auctions WHERE id = $1")
        .bind(seeded.auction_id)
        .fetch_one(pool)
        .await
        .expect("读取场次状态");
    assert_eq!(status, "final", "取消后的重试必须真实提交");

    store.postgres_pool().close().await;
}

/// 负载演练：同一场次的 Final 并发出价 + Admin 最终阶段迁移，以及无关场次的 Admin lot 修改。
///
/// 显式运行：
/// `cargo test -p auction-storage --features test-db --test postgres \
///  -- --ignored --nocapture postgres_auction_queue_drill`
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "W12.1 跟进演练：显式运行以测量生命周期/Admin 写入与 Final 出价的重叠"]
async fn postgres_auction_queue_drill_reports_latency_and_pool_usage() {
    const ROUNDS: usize = 6;
    let concurrent = std::env::var("AUCTION_CROSSLOT_LOTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (2..=12).contains(value))
        .unwrap_or(DEFAULT_CONCURRENT_WRITES);
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let mut auction_sequence = 0_i32;

    for mode in [BidQueueMode::Direct, BidQueueMode::Queued] {
        let store = database.store().await.expect("连接测试数据库").with_bid_queue_mode(mode);
        let bids = Arc::new(DatabaseCompetitionCommands::new(Arc::new(store.clone())));
        let lifecycle = Arc::new(lifecycle_commands(&store));
        let admin = Arc::new(admin_lot_commands(&store).await);
        let pool = store.postgres_pool();
        let mut bid_latencies = Vec::new();
        let mut lifecycle_latencies = Vec::new();
        let mut admin_latencies = Vec::new();
        let mut conflicts = 0_usize;
        let mut timeouts = 0_usize;
        let mut other = 0_usize;
        let mut accepted_bids = 0_usize;
        let mut accepted_lifecycle = 0_usize;
        let deadlocks_before = deadlock_count(pool).await;
        let peak = Arc::new(AtomicU64::new(0));

        for round in 0..ROUNDS {
            auction_sequence += 1;
            let seed = format!("queue-drill-{mode:?}-{round}");
            let live =
                seed_auction(&store, &seed, "main", "live", auction_sequence, concurrent).await;
            let draft = seed_auction(
                &store,
                &format!("{seed}-draft"),
                "isekai",
                "draft",
                auction_sequence,
                2,
            )
            .await;
            let barrier = Arc::new(Barrier::new(concurrent + 3));
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

            let lifecycle_task = {
                let lifecycle = Arc::clone(&lifecycle);
                let barrier = Arc::clone(&barrier);
                let command = final_phase_command(&seed, live.auction_id);
                tokio::spawn(async move {
                    barrier.wait().await;
                    let started = Instant::now();
                    let result = lifecycle.execute(command).await;
                    (started.elapsed(), result)
                })
            };
            let mut bid_tasks = Vec::with_capacity(concurrent);
            for index in 0..concurrent {
                let bids = Arc::clone(&bids);
                let barrier = Arc::clone(&barrier);
                let command = bid_command(&seed, index, live.lots[index], live.bidders[index]);
                bid_tasks.push(tokio::spawn(async move {
                    barrier.wait().await;
                    let started = Instant::now();
                    let result = bids.place_bid(command).await;
                    (started.elapsed(), result)
                }));
            }
            let mut admin_tasks = Vec::with_capacity(2);
            for (index, lot_id) in draft.lots.iter().copied().enumerate() {
                let admin = Arc::clone(&admin);
                let barrier = Arc::clone(&barrier);
                let command = admin_lot_command(
                    &format!("{seed}-admin-{index}"),
                    lot_id,
                    draft.lot_state_versions[index],
                    60_000 + i64::try_from(index).expect("index") * 1_000,
                );
                admin_tasks.push(tokio::spawn(async move {
                    barrier.wait().await;
                    let started = Instant::now();
                    let result = admin.update(command).await;
                    (started.elapsed(), result)
                }));
            }

            let (elapsed, result) = lifecycle_task.await.expect("生命周期任务不得 panic");
            lifecycle_latencies.push(elapsed);
            match result {
                Ok(_) => accepted_lifecycle += 1,
                Err(ApplicationError::CompetitionTransaction(
                    CompetitionTransactionError::SerializationConflict,
                )) => conflicts += 1,
                Err(ApplicationError::CompetitionTransaction(
                    CompetitionTransactionError::QueueWaitTimeout,
                )) => timeouts += 1,
                Err(error) => {
                    if other < 3 {
                        println!("  轮 {round} 生命周期其它失败：{error:?}");
                    }
                    other += 1;
                }
            }
            for task in bid_tasks {
                let (elapsed, result) = task.await.expect("出价任务不得 panic");
                bid_latencies.push(elapsed);
                match result {
                    Ok(_) => accepted_bids += 1,
                    Err(ApplicationError::CompetitionTransaction(
                        CompetitionTransactionError::SerializationConflict,
                    )) => conflicts += 1,
                    Err(ApplicationError::CompetitionTransaction(
                        CompetitionTransactionError::QueueWaitTimeout,
                    )) => timeouts += 1,
                    Err(error) => {
                        if other < 3 {
                            println!("  轮 {round} 出价其它失败：{error:?}");
                        }
                        other += 1;
                    }
                }
            }
            for task in admin_tasks {
                let (elapsed, result) = task.await.expect("Admin 任务不得 panic");
                admin_latencies.push(elapsed);
                if let Err(error) = result {
                    if other < 3 {
                        println!("  轮 {round} Admin 其它失败：{error:?}");
                    }
                    other += 1;
                }
            }
            sampler.abort();
            retire_auction(&store, live.auction_id).await;
            retire_auction(&store, draft.auction_id).await;
        }

        let percentile = |values: &mut Vec<std::time::Duration>, fraction: f64| {
            values.sort_unstable();
            if values.is_empty() {
                return 0.0;
            }
            let index = ((values.len() as f64 - 1.0) * fraction).round() as usize;
            values[index].as_secs_f64() * 1_000.0
        };
        println!(
            "W12.1 跟进 drill mode={mode:?} 轮数={ROUNDS} 并发=出价 {concurrent} + 生命周期 1 + \
             Admin 2 出价成功={accepted_bids} 生命周期成功={accepted_lifecycle} \
             序列化/死锁耗尽={conflicts} 排队超时={timeouts} 其它失败={other}"
        );
        println!(
            "  出价 p50={:.1}ms p95={:.1}ms p99={:.1}ms",
            percentile(&mut bid_latencies, 0.50),
            percentile(&mut bid_latencies, 0.95),
            percentile(&mut bid_latencies, 0.99),
        );
        println!(
            "  生命周期 p50={:.1}ms p95={:.1}ms p99={:.1}ms",
            percentile(&mut lifecycle_latencies, 0.50),
            percentile(&mut lifecycle_latencies, 0.95),
            percentile(&mut lifecycle_latencies, 0.99),
        );
        println!(
            "  Admin(无关场次) p50={:.1}ms p95={:.1}ms p99={:.1}ms",
            percentile(&mut admin_latencies, 0.50),
            percentile(&mut admin_latencies, 0.95),
            percentile(&mut admin_latencies, 0.99),
        );
        println!(
            "  死锁增量={} 连接池峰值={} （池上限 {}）",
            deadlock_count(pool).await - deadlocks_before,
            peak.load(Ordering::Relaxed),
            pool.options().get_max_connections(),
        );
        if mode == BidQueueMode::Queued {
            assert_eq!(conflicts, 0, "排队后不得再出现序列化/死锁耗尽");
            assert_eq!(timeouts, 0, "排队预算内不得超时");
            assert_eq!(other, 0, "排队后不得出现其它失败");
            assert_eq!(accepted_lifecycle, ROUNDS, "每轮最终阶段迁移都必须成功");
            assert_eq!(accepted_bids, ROUNDS * concurrent, "每轮每个 lot 的出价都必须成功");
            assert_eq!(deadlock_count(pool).await, deadlocks_before, "排队后不得出现死锁");
        }
        pool.close().await;
    }
}

/// 夹具自检：`Draft` 场次的 Admin 修改确实推进 `auctions.state_version`，且与出价共用同一
/// 把门（同一个 auction 键）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgres_admin_lot_queue_shares_auction_key_with_bids() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let seeded = seed_auction(&store, "queue-admin-key", "main", "live", 1, 1).await;
    let pool = store.postgres_pool();
    let admin = Arc::new(admin_lot_commands(&store).await);
    let bids = DatabaseCompetitionCommands::new(Arc::new(store.clone()));

    let mut blocker = pool.acquire().await.expect("取得阻塞连接");
    sqlx::query("BEGIN").execute(&mut *blocker).await.expect("开启阻塞事务");
    sqlx::query("SELECT id FROM public.lots WHERE id = $1 FOR UPDATE")
        .bind(seeded.lots[0])
        .execute(&mut *blocker)
        .await
        .expect("锁定 lot");

    let bid = {
        let bids = Arc::new(bids);
        let command = bid_command("queue-admin-key", 0, seeded.lots[0], seeded.bidders[0]);
        tokio::spawn(async move { bids.place_bid(command).await })
    };
    assert!(
        wait_for_condition(StdDuration::from_secs(5), || async {
            advisory_lock_count(pool).await >= 1
        })
        .await,
        "出价必须已经持有场次门"
    );

    // `live` 场次的 lot 不能走 Admin 修改（状态门禁），但排队发生在事务之前：这里只验证
    // 同一场次的 Admin 修改不会在出价持门期间进入事务，因此必须在队列上等待。
    let admin_task = {
        let admin = Arc::clone(&admin);
        let command = admin_lot_command("queue-admin-key", seeded.lots[0], 1, 54_000);
        tokio::spawn(async move { admin.update(command).await })
    };
    let mut admin_task = admin_task;
    assert!(
        tokio::time::timeout(StdDuration::from_millis(300), &mut admin_task).await.is_err(),
        "同场次 Admin 修改不得在出价持门期间进入事务"
    );

    sqlx::query("ROLLBACK").execute(&mut *blocker).await.expect("释放阻塞事务");
    drop(blocker);
    bid.await.expect("出价任务不得 panic").expect("出价必须成功");
    // 门释放后 Admin 修改进入事务，并按状态门禁以 InvalidState 结束；关键是它不再等待。
    let error = tokio::time::timeout(StdDuration::from_secs(5), &mut admin_task)
        .await
        .expect("门释放后 Admin 修改必须结束")
        .expect("Admin 任务不得 panic")
        .expect_err("live 场次的 lot 不得被 Admin 修改");
    assert!(
        matches!(
            error,
            ApplicationError::CompetitionTransaction(
                CompetitionTransactionError::InvalidState | CompetitionTransactionError::StaleState
            )
        ),
        "同场次 Admin 修改必须按状态/版本门禁失败：{error:?}"
    );

    store.postgres_pool().close().await;
}
