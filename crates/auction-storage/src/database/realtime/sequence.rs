//! Realtime stream 序号分配；各 owner 追加公开事件时共用同一条持锁 upsert。

use chrono::{DateTime, Utc};

/// 在当前事务内为 stream 分配下一个连续序号，并持有该 stream 行锁直到事务结束。
/// 首次写入时创建 stream 并占用序号 1，已有 stream 则原子递增 `next_sequence`。单条
/// `ON CONFLICT DO UPDATE` 与“补建 → `FOR UPDATE` → 递增”取得同一行锁，Serializable 并发更新
/// 冲突仍报告序列化失败；只减少持锁期间的往返。`updated_at` 为 `None` 时使用事务时间。
pub(crate) async fn allocate_stream_sequence(
    connection: &mut sqlx::PgConnection,
    stream_key: &str,
    updated_at: Option<DateTime<Utc>>,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO public.realtime_streams AS stream (stream_key, next_sequence, updated_at)
         VALUES ($1, 2, COALESCE($2, CURRENT_TIMESTAMP))
         ON CONFLICT (stream_key) DO UPDATE
         SET next_sequence = stream.next_sequence + 1,
             updated_at = COALESCE($2, CURRENT_TIMESTAMP)
         RETURNING stream.next_sequence - 1",
    )
    .bind(stream_key)
    .bind(updated_at)
    .fetch_one(connection)
    .await
}
