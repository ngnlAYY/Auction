//! W12.1 提交后序号分配（D5 根治）的 PostgreSQL 18 集成测试。
//!
//! 只写本测试克隆库的 public 表；夹具见 `tests/support`。覆盖：
//! - 待定事件按提交顺序补上连续序号，重复调用幂等，计数行尾部一致；
//! - 同一场次、不同 lot 的两次 Serializable 事务只写事件行（不读写共享计数行）时都不中止；
//! - 迁移 000086 只允许把 `stream_sequence` 从 NULL 分配一次，事件内容不可改写。
#![cfg(feature = "test-db")]
#![allow(clippy::expect_used)]

use auction_application::RealtimeSequenceAllocator;
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
