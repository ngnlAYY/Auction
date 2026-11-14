use std::{
    future::Future,
    task::{Context, Poll},
};

use super::*;

fn lazy_pool() -> Pool<sqlx::Postgres> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_lazy("postgres://auction@127.0.0.1:1/auction")
        .unwrap_or_else(|error| panic!("惰性连接池只解析 URL：{error}"))
}

/// 出价队列连接配额是普通配额的四分之一（1..=4），且不超过物理池的剩余位。
#[test]
fn bid_queue_capacity_is_bounded_by_a_quarter_and_by_spare_connections() {
    assert_eq!(super::bid_queue_capacity(1, 16), 1);
    assert_eq!(super::bid_queue_capacity(3, 16), 1);
    // 普通 14 + 长期 owner 1 + readiness 1 = 物理 16，只余 2 位。
    assert_eq!(super::bid_queue_capacity(14, 16), 2);
    assert_eq!(super::bid_queue_capacity(64, 128), 4);
    // 物理池没有余位（非标准档）时仍保留 1 条，最坏由 acquire 时限收口。
    assert_eq!(super::bid_queue_capacity(16, 16), 1);
}

/// 普通业务配额耗尽后必须停在上限处；readiness 不等待该配额，直接走真实池。
#[tokio::test]
async fn exhausted_capacity_does_not_block_readiness_or_reserved_owners() {
    let pool = TrackedPool::new(lazy_pool(), 1);
    assert_eq!(pool.ordinary_capacity(), 1);
    let held = pool.ordinary.clone().try_acquire_owned().expect("取得唯一普通配额");
    let waker = futures_util::task::noop_waker();
    let mut context = Context::from_waker(&waker);

    let mut ordinary = Box::pin(pool.acquire());
    assert!(matches!(ordinary.as_mut().poll(&mut context), Poll::Pending));
    assert!(ordinary.inner.is_none(), "配额耗尽时不得开始真实池获取");

    let mut readiness = Box::pin(pool.acquire_readiness());
    let _ = readiness.as_mut().poll(&mut context);
    assert!(readiness.permit_future.is_none(), "readiness 不等待普通配额");
    assert!(readiness.inner.is_some(), "readiness 必须直接进入真实池获取");
    assert_eq!(pool.ordinary.available_permits(), 0, "readiness 不消耗普通配额");

    let mut reserved = Box::pin(pool.acquire_reserved(DatabaseReservedPurpose::RealtimeOwnership));
    let _ = reserved.as_mut().poll(&mut context);
    assert!(reserved.inner.is_some(), "长期 owner 同样保留物理连接");

    drop(held);
    drop(ordinary);
    drop(readiness);
    drop(reserved);
}

/// 普通配额饱和时的等待计入池的 acquire 时限：到期按池超时失败并记入 Timeout 结果，
/// 不再挂到 HTTP 或 Worker 超时；配额门之前不会触碰物理池。
#[tokio::test(start_paused = true)]
async fn ordinary_capacity_wait_is_bounded_by_the_acquire_timeout() {
    let raw = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("postgres://auction@127.0.0.1:1/auction")
        .unwrap_or_else(|error| panic!("惰性连接池只解析 URL：{error}"));
    let metrics = auction_observability::ProcessMetrics::new(true, "auction", "worker", "test")
        .expect("创建测试指标注册表");
    let pool = TrackedPool::new(raw, 1).with_database_recorder(metrics.database());
    let held = pool.ordinary.clone().try_acquire_owned().expect("取得唯一普通配额");

    let started = tokio::time::Instant::now();
    // 循环归属在创建 acquisition 时读取，因此必须在作用域内创建 future；外层时限只把回归时的
    // 无限等待变成失败，正常路径由 50ms 的 acquire 时限先结束。
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        auction_observability::scope_database_worker_loop(
            auction_observability::WorkerLoop::Delivery,
            async { pool.acquire().await },
        ),
    )
    .await
    .expect("配额等待必须在 acquire 时限内结束");
    assert!(matches!(result, Err(sqlx::Error::PoolTimedOut)), "配额等待超时必须按池超时失败");
    assert!(started.elapsed() >= std::time::Duration::from_millis(50));
    let exposition = String::from_utf8(metrics.encode().expect("编码指标")).expect("指标为 UTF-8");
    assert!(
        exposition.contains(
            r#"auction_worker_db_acquire_duration_seconds_count{loop="delivery",outcome="timeout"} 1"#
        ),
        "{exposition}"
    );
    assert_eq!(pool.ordinary.available_permits(), 0, "超时不得越过或泄漏配额");
    drop(held);
    assert_eq!(pool.ordinary.available_permits(), 1);
}

/// 出价队列配额等待同样受 acquire 时限约束；时限先于 5 秒排队预算到期时，对调用方仍是
/// 排队等待超时（QueueWaitTimeout 与 timed_out 结果），不得降级为通用失败。
#[tokio::test(start_paused = true)]
async fn bid_queue_capacity_timeout_is_a_queue_wait_timeout() {
    let raw = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("postgres://auction@127.0.0.1:1/auction")
        .unwrap_or_else(|error| panic!("惰性连接池只解析 URL：{error}"));
    let metrics = auction_observability::ProcessMetrics::new(true, "auction", "public", "test")
        .expect("创建测试指标注册表");
    let pool = TrackedPool::new(raw, 1).with_database_recorder(metrics.database());
    let capacity = u32::try_from(pool.bid_queue.available_permits()).expect("配额可放进 u32");
    let _held = pool.bid_queue.clone().try_acquire_many_owned(capacity).expect("占满出价队列配额");
    let queue =
        std::sync::Arc::new(crate::database::auction::competition::AuctionBidQueue::default());

    let started = tokio::time::Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        queue.enqueue_auction(&pool, uuid::Uuid::from_u128(1)),
    )
    .await
    .expect("配额等待必须在 acquire 时限内结束");
    assert!(
        matches!(
            result,
            Err(auction_application::core::CompetitionTransactionError::QueueWaitTimeout)
        ),
        "配额超时必须按排队等待超时分类"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "应由 acquire 时限先结束");
    let exposition = String::from_utf8(metrics.encode().expect("编码指标")).expect("指标为 UTF-8");
    assert!(
        exposition.contains(r#"bid_queue_requests_total{outcome="timed_out"} 1"#),
        "{exposition}"
    );
    assert!(!exposition.contains(r#"bid_queue_requests_total{outcome="failed"}"#), "{exposition}");
}
