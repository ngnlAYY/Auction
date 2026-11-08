//! W12.1 提交后序号分配（D5 根治）的 PostgreSQL 18 集成测试。
//!
//! 只写本测试克隆库的 public 表；夹具见 `tests/support`。覆盖：
//! - 待定事件按提交顺序补上连续序号，重复调用幂等，计数行尾部一致；
//! - 同一场次、不同 lot 的两次 Serializable 事务只写事件行（不读写共享计数行）时都不中止；
//! - 迁移 000086 只允许把 `stream_sequence` 从 NULL 分配一次，事件内容不可改写。
#![cfg(feature = "test-db")]
#![allow(clippy::expect_used)]

use auction_application::realtime::RealtimeSequenceAllocator;
use sqlx::{Connection, Row};
use uuid::Uuid;

use crate::support;

async fn seed_stream(pool: &sqlx::PgPool, stream_key: &str) {
    sqlx::query("INSERT INTO public.realtime_streams (stream_key, next_sequence) VALUES ($1, 1)")
        .bind(stream_key)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("建立测试 stream: {error}"));
}

async fn insert_pending_event(pool: &sqlx::PgPool, stream_key: &str, created_at: &str) -> Uuid {
    let event_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload, created_at)
         VALUES ($1, $2, NULL, 'auction.opened', 1, '{\"auction_id\": null}'::jsonb,
                 transaction_timestamp() + $3::interval)",
    )
    .bind(event_id)
    .bind(stream_key)
    .bind(created_at)
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("写入待定事件: {error}"));
    event_id
}

async fn ordered_sequences(pool: &sqlx::PgPool, stream_key: &str) -> Vec<(Uuid, Option<i64>)> {
    sqlx::query(
        "SELECT event_id, stream_sequence FROM public.realtime_events
         WHERE stream_key = $1 ORDER BY created_at, event_id",
    )
    .bind(stream_key)
    .fetch_all(pool)
    .await
    .expect("读取事件序号")
    .into_iter()
    .map(|row| (row.get("event_id"), row.get("stream_sequence")))
    .collect()
}

#[tokio::test]
async fn postgres_deferred_sequences_are_contiguous_and_idempotent() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let pool = store.postgres_pool().clone();
    let stream_key = format!("auction:{}", Uuid::new_v4());
    seed_stream(&pool, &stream_key).await;

    // 故意乱序插入：分配器只按 (created_at, event_id) 顺序补号。
    let second = insert_pending_event(&pool, &stream_key, "1 second").await;
    let first = insert_pending_event(&pool, &stream_key, "0 seconds").await;
    let third = insert_pending_event(&pool, &stream_key, "2 seconds").await;

    let allocated =
        store.allocate_pending_sequences("auction:".to_owned()).await.expect("补齐待定序号");
    assert_eq!(allocated, 3, "本轮应分配全部三条待定事件");
    let ordered = ordered_sequences(&pool, &stream_key).await;
    assert_eq!(
        ordered,
        vec![(first, Some(1)), (second, Some(2)), (third, Some(3))],
        "序号必须按提交顺序连续，且不因插入顺序改变"
    );
    let next_sequence: i64 = sqlx::query_scalar(
        "SELECT next_sequence FROM public.realtime_streams WHERE stream_key = $1",
    )
    .bind(&stream_key)
    .fetch_one(&pool)
    .await
    .expect("读取计数行");
    assert_eq!(next_sequence, 4, "计数行只推进已分配条数");

    // 幂等：没有待定事件时不再推进计数行。
    let again = store.allocate_pending_sequences("auction:".to_owned()).await.expect("重复补齐");
    assert_eq!(again, 0);
    let next_sequence: i64 = sqlx::query_scalar(
        "SELECT next_sequence FROM public.realtime_streams WHERE stream_key = $1",
    )
    .bind(&stream_key)
    .fetch_one(&pool)
    .await
    .expect("读取计数行");
    assert_eq!(next_sequence, 4);

    pool.close().await;
}

/// 同场不同 lot 的重叠出价：两次 Serializable 事务都只写待定事件行，不再读写共享计数行。
/// 旧实现在事务内 upsert 同一 `realtime_streams` 行，必有一方以 40001 中止（D5）。
#[tokio::test]
async fn postgres_deferred_bid_events_do_not_abort_under_serializable() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let pool = store.postgres_pool().clone();
    let stream_key = format!("auction:{}", Uuid::new_v4());
    seed_stream(&pool, &stream_key).await;

    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let mut handles = Vec::new();
    for offset in ["0 seconds", "1 second"] {
        let pool = pool.clone();
        let stream_key = stream_key.clone();
        let barrier = std::sync::Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            let mut connection = pool.acquire().await.expect("取得测试连接");
            let mut transaction =
                connection.begin().await.map_err(|error| error.to_string()).expect("开始事务");
            sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
                .execute(&mut *transaction)
                .await
                .expect("设置隔离级别");
            // 与竞价事务相同的写入形状：只 INSERT 事件行（stream_sequence 为 NULL）。
            sqlx::query(
                "INSERT INTO public.realtime_events
                    (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
                     created_at)
                 VALUES ($1, $2, NULL, 'auction.opened', 1, '{\"auction_id\": null}'::jsonb,
                         transaction_timestamp() + $3::interval)",
            )
            .bind(Uuid::new_v4())
            .bind(&stream_key)
            .bind(offset)
            .execute(&mut *transaction)
            .await
            .unwrap_or_else(|error| panic!("写入待定事件: {error}"));
            barrier.wait().await;
            transaction.commit().await.map_err(|error| error.to_string())
        }));
    }
    for handle in handles {
        handle.await.expect("事务任务不得 panic").expect("两次重叠事务都必须提交成功");
    }

    let allocated =
        store.allocate_pending_sequences("auction:".to_owned()).await.expect("补齐待定序号");
    assert_eq!(allocated, 2);
    let sequences = ordered_sequences(&pool, &stream_key).await;
    assert_eq!(
        sequences.iter().map(|(_, sequence)| *sequence).collect::<Vec<_>>(),
        vec![Some(1), Some(2)],
        "提交后补号仍保持连续"
    );

    pool.close().await;
}

/// 迁移 000086：序号只能从 NULL 分配一次，事件内容不可被 UPDATE 改写。
#[tokio::test]
async fn postgres_deferred_sequence_only_assigns_once() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let pool = store.postgres_pool().clone();
    let stream_key = format!("auction:{}", Uuid::new_v4());
    seed_stream(&pool, &stream_key).await;
    let event_id = insert_pending_event(&pool, &stream_key, "0 seconds").await;

    let content_rewrite = sqlx::query(
        "UPDATE public.realtime_events SET kind = 'auction.settled' WHERE event_id = $1",
    )
    .bind(event_id)
    .execute(&pool)
    .await;
    assert!(content_rewrite.is_err(), "事件内容不可改写");

    sqlx::query("UPDATE public.realtime_events SET stream_sequence = 1 WHERE event_id = $1")
        .bind(event_id)
        .execute(&pool)
        .await
        .expect("首次分配序号");
    let reassign =
        sqlx::query("UPDATE public.realtime_events SET stream_sequence = 2 WHERE event_id = $1")
            .bind(event_id)
            .execute(&pool)
            .await;
    assert!(reassign.is_err(), "已分配序号不得再次分配");

    pool.close().await;
}

/// 同一命令的多条竞价事实（如出价后自动代理反击）共享 `created_at = transaction_timestamp()`。
/// 补号必须按已接受事实的 `state_version`/`origin_fact_index` 顺序，不能由随机 `event_id`
/// 决定先后：否则 `RealtimeCache::merge_accepted_bid` 会把连续事实判成缺口并强制整页刷新。
#[tokio::test]
async fn postgres_deferred_sequences_keep_the_accepted_bid_fact_order() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let pool = store.postgres_pool().clone();
    let stream_key = format!("auction:{}", Uuid::new_v4());
    seed_stream(&pool, &stream_key).await;
    let facts = seed_bid_facts(&pool).await;

    // 已接受顺序是 fact 1 → fact 2；把较小的 UUID 分给较晚的事实，旧实现（按 event_id
    // 排序）会把它们颠倒。
    let (first_event, second_event) = reverse_ordered_event_ids();
    for (event_id, fact_index) in [(first_event, 1_i32), (second_event, 2_i32)] {
        sqlx::query(
            "INSERT INTO public.realtime_events
                (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
                 state_version, origin_command_id, origin_bid_id, origin_fact_index, created_at)
             VALUES ($1, $2, NULL, 'lot.bid_accepted', 2, '{}'::jsonb, $3, $4, $5, $6,
                     '2026-01-01 00:00:00+00'::timestamptz)",
        )
        .bind(event_id)
        .bind(&stream_key)
        .bind(i64::from(fact_index))
        .bind(facts.command_id)
        .bind(if fact_index == 1 { facts.first_bid } else { facts.second_bid })
        .bind(fact_index)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("写入待定竞价事件: {error}"));
    }

    let allocated =
        store.allocate_pending_sequences("auction:".to_owned()).await.expect("补齐待定序号");
    assert_eq!(allocated, 2, "本轮应分配两条待定竞价事件");
    let sequences: Vec<(i32, i64)> = sqlx::query(
        "SELECT origin_fact_index, stream_sequence FROM public.realtime_events
         WHERE stream_key = $1 ORDER BY stream_sequence",
    )
    .bind(&stream_key)
    .fetch_all(&pool)
    .await
    .expect("读取事件序号")
    .into_iter()
    .map(|row| (row.get("origin_fact_index"), row.get("stream_sequence")))
    .collect();
    assert_eq!(
        sequences,
        vec![(1, 1), (2, 2)],
        "共享时间戳的竞价事实必须保持接受顺序，而不是随机 event_id 顺序"
    );

    pool.close().await;
}

struct BidFacts {
    command_id: Uuid,
    first_bid: Uuid,
    second_bid: Uuid,
}

/// 一条命令内的两条已接受竞价事实（fact 1 → state_version 1，fact 2 → state_version 2）。
async fn seed_bid_facts(pool: &sqlx::PgPool) -> BidFacts {
    let seed = Uuid::new_v4().to_string();
    let user_id = Uuid::new_v4();
    let mut public_key_hash = [0_u8; 32];
    public_key_hash[..16].copy_from_slice(user_id.as_bytes());
    public_key_hash[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    sqlx::query("INSERT INTO public.users (id, username, public_key_hash) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(format!("bid-order-{seed}"))
        .bind(public_key_hash.as_slice())
        .execute(pool)
        .await
        .expect("写入测试用户");

    let auction_id = Uuid::new_v4();
    let equipment_id = Uuid::new_v4();
    let lot_id = Uuid::new_v4();
    let policy = serde_json::json!({
        "schema_version": 5,
        "bidding_version": 2,
        "start_price": 50000,
        "bid_increment_ppm": 0,
        "minimum_increment": 1000,
        "soft_close_window_seconds": 900,
        "soft_close_extension_seconds": 900,
        "proxy_enabled": true,
        "proxy_default_mode": "fast",
        "proxy_slow_interval_seconds": 600,
        "proxy_max_outbid_uses": 0,
        "final_phase_lead_seconds": 900,
        "scheduled_end_at": "2026-01-01T02:00:00Z",
        "settlement": {
            "version": 1,
            "mode": "hold_curve_v1",
            "compensation_pool_enabled": true,
            "anchors": []
        },
        "recovery": {"version": 1, "mode": "remind_v1"}
    });
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, policy_snapshot)
         VALUES ($1, 'main', 1, $2, 'live', '2026-01-01T00:00:00+00',
                 '2026-01-01T00:00:00+00', '2026-01-01T02:00:00+00',
                 '2026-01-01T02:00:00+00', $3)",
    )
    .bind(auction_id)
    .bind(format!("{seed} auction"))
    .bind(&policy)
    .execute(pool)
    .await
    .expect("写入测试场次");
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, collected_at)
         VALUES ($1, 'main', $2, $3, 'weapon', '2026-01-01T00:00:00+00')",
    )
    .bind(equipment_id)
    .bind(format!("{seed}-eid"))
    .bind(format!("{seed} equipment"))
    .execute(pool)
    .await
    .expect("写入测试装备");
    sqlx::query(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
             leader_user_id, effective_end_at, category_key, category_number)
         VALUES ($1, $2, $3, 1, 'live', 50000, 120000, $4,
                 '2026-01-01T02:00:00+00', 'weapon', 1)",
    )
    .bind(lot_id)
    .bind(auction_id)
    .bind(equipment_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("写入测试 lot");

    let command_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'place_bid', $2, $3, 'accepted')",
    )
    .bind(command_id)
    .bind(auction_id)
    .bind([7_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入测试命令");

    let first_bid = Uuid::new_v4();
    let second_bid = Uuid::new_v4();
    for (bid_id, fact_index) in [(first_bid, 1_i32), (second_bid, 2_i32)] {
        sqlx::query(
            "INSERT INTO public.bids
                (id, command_id, fact_index, lot_id, bidder_user_id, amount, mechanism,
                 effective_end_at, accepted_state_version)
             VALUES ($1, $2, $3, $4, $5, $6, 'ordinary',
                     '2026-01-01T02:00:00+00', $7)",
        )
        .bind(bid_id)
        .bind(command_id)
        .bind(fact_index)
        .bind(lot_id)
        .bind(user_id)
        .bind(120_000_i64 + i64::from(fact_index))
        .bind(i64::from(fact_index))
        .execute(pool)
        .await
        .expect("写入测试出价事实");
    }

    BidFacts { command_id, first_bid, second_bid }
}

/// 返回 `(较晚事实的事件 ID, 较早事实的事件 ID)`，并保证前者 UUID 更小。
fn reverse_ordered_event_ids() -> (Uuid, Uuid) {
    loop {
        let earlier = Uuid::new_v4();
        let later = Uuid::new_v4();
        if later < earlier {
            return (earlier, later);
        }
    }
}
