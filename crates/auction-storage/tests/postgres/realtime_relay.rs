//! Public relay 持久化消费端口与 Admin 滞后视图的 PostgreSQL 18 集成测试。
//!
//! 只写本测试克隆库的 public 表；夹具见 `tests/support`。覆盖：
//! - 迁移 000011 留下的 `auction:discovery` 历史发现流只保留、没有读者：即使仍有超过游标的
//!   事件，也不出现在 relay 的待转发目录中；单拍卖流照常列出。
//! - 该流上落后头部的旧游标（升级前旧版本留下、此后再无路径推进）不计入 Admin 仪表盘的全量
//!   积压，也不进入滞后列表的计数与明细；单拍卖流的滞后照常计入。
#![allow(clippy::expect_used)]

use auction_application::realtime::{
    MAX_REALTIME_ADMIN_PAGE_SIZE, RealtimeAdminPort, RealtimeLagListRequest, RealtimeStreamStore,
};
use uuid::Uuid;

use crate::support;

/// 以立即取号的形状追加一条已分配序号的事件：同一夹具事务内推进计数行并写入事件，返回其序号。
async fn append_committed_event(pool: &sqlx::PgPool, stream_key: &str) -> i64 {
    let mut transaction = pool.begin().await.expect("开启夹具事务");
    let sequence: i64 = sqlx::query_scalar(
        "INSERT INTO public.realtime_streams AS stream (stream_key, next_sequence)
         VALUES ($1, 2)
         ON CONFLICT (stream_key) DO UPDATE SET next_sequence = stream.next_sequence + 1
         RETURNING next_sequence - 1",
    )
    .bind(stream_key)
    .fetch_one(&mut *transaction)
    .await
    .expect("推进测试 stream 计数行");
    sqlx::query(
        "INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload)
         VALUES ($1, $2, $3, 'auction.created', 1, '{}'::jsonb)",
    )
    .bind(Uuid::new_v4())
    .bind(stream_key)
    .bind(sequence)
    .execute(&mut *transaction)
    .await
    .expect("写入测试事件");
    transaction.commit().await.expect("提交夹具事务");
    sequence
}

/// 在流内追加两条事件，并把 consumer 游标停在第一条：该 consumer 在此流上落后头部 1 条。
async fn append_lagging_offset(pool: &sqlx::PgPool, consumer_key: &str, stream_key: &str) {
    let first = append_committed_event(pool, stream_key).await;
    append_committed_event(pool, stream_key).await;
    sqlx::query(
        "INSERT INTO public.realtime_consumer_offsets (consumer_key, stream_key, last_sequence)
         VALUES ($1, $2, $3)",
    )
    .bind(consumer_key)
    .bind(stream_key)
    .bind(first)
    .execute(pool)
    .await
    .expect("写入落后头部的测试游标");
}

fn lag_request(stream_prefix: Option<&str>) -> RealtimeLagListRequest {
    RealtimeLagListRequest {
        stream_prefix: stream_prefix.map(str::to_owned),
        consumer_key: None,
        limit: MAX_REALTIME_ADMIN_PAGE_SIZE,
        offset: 0,
    }
}

#[tokio::test]
async fn postgres_realtime_relay_directory_skips_the_retired_discovery_stream() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let pool = store.postgres_pool().clone();
    let auction_stream = format!("auction:{}", Uuid::new_v4());
    append_committed_event(&pool, "auction:discovery").await;
    append_committed_event(&pool, &auction_stream).await;

    // 新 consumer 没有任何游标，缺失游标按 0 处理：单拍卖流待转发，历史发现流不被列出，
    // 因此 relay 不会读取保留期内的历史发现事件并把它们记成失败。
    let consumer_key = "relay-directory-test";
    let ownership = store
        .acquire_realtime_consumer_ownership(consumer_key)
        .await
        .expect("取得 consumer 所有权");
    let pending = ownership
        .list_pending_stream_keys(
            consumer_key.to_owned(),
            "auction:".to_owned(),
            String::new(),
            200,
        )
        .await
        .expect("读取待转发 stream 目录");
    assert_eq!(pending, vec![auction_stream]);

    drop(ownership);
    pool.close().await;
}

#[tokio::test]
async fn postgres_realtime_admin_lag_views_skip_the_retired_discovery_stream() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接测试数据库");
    let pool = store.postgres_pool().clone();
    let auction_stream = format!("auction:{}", Uuid::new_v4());
    // 升级前的旧 relay 在两条流上都落后头部 1 条；新版本没有任何路径再推进发现流游标。
    let consumer_key = "relay-lag-test";
    append_lagging_offset(&pool, consumer_key, "auction:discovery").await;
    append_lagging_offset(&pool, consumer_key, &auction_stream).await;
    let expected = vec![(consumer_key, auction_stream.as_str(), 1_i64)];

    // 仪表盘全量积压只计单拍卖流，有界明细同口径，`page_lag <= total_lag` 校验照常成立；
    // 否则这条永不推进的旧游标会让实时分区永久降级。
    let snapshot =
        RealtimeAdminPort::dashboard_snapshot(&store).await.expect("读取 realtime 仪表盘");
    assert_eq!(snapshot.total_lag, 1);
    let dashboard_lags: Vec<_> = snapshot
        .lags
        .iter()
        .map(|lag| (lag.consumer_key.as_str(), lag.stream_key.as_str(), lag.lag))
        .collect();
    assert_eq!(dashboard_lags, expected);

    // 滞后列表的计数与明细同口径：总数不多出明细取不到的发现流行。
    let page = RealtimeAdminPort::list_lag(&store, lag_request(None))
        .await
        .expect("读取 realtime 滞后列表");
    assert_eq!(page.total, 1);
    let listed: Vec<_> = page
        .entries
        .iter()
        .map(|lag| (lag.consumer_key.as_str(), lag.stream_key.as_str(), lag.lag))
        .collect();
    assert_eq!(listed, expected);

    // 按发现流前缀筛选得到空页，而不是那条落后游标。
    let discovery = RealtimeAdminPort::list_lag(&store, lag_request(Some("auction:discovery")))
        .await
        .expect("按发现流前缀读取滞后列表");
    assert_eq!(discovery.total, 0);
    assert!(discovery.entries.is_empty());

    pool.close().await;
}
