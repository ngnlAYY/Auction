use self::parent::*;
use crate::database::pool as parent;
use std::{
    future::Future,
    task::{Context, Poll},
};

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
