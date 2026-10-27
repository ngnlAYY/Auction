//! Realtime stream 目录查询适配器。

use auction_application::ApplicationError;

use super::common::storage_error;
pub(super) fn validate_stream_directory_query(
    prefix: &str,
    after: &str,
    limit: u16,
) -> Result<(), ApplicationError> {
    if prefix != "auction:"
        || after.len() > 255
        || after != after.trim()
        || after.chars().any(char::is_control)
        || limit == 0
        || limit > 200
    {
        return Err(storage_error("实时 stream 目录查询参数无效"));
    }
    Ok(())
}

/// 只列出头部序号超过该 consumer 游标的 stream。`next_sequence - 1` 由所有事件追加路径在
/// 同一事务内持锁推进，是 stream 的权威头部；缺失游标按 0 处理，与批量读取一致。
#[cfg(feature = "postgres")]
pub(super) async fn list_pending_streams_postgres(
    connection: &mut sqlx::PgConnection,
    consumer_key: &str,
    prefix: &str,
    after: &str,
    limit: u16,
) -> Result<Vec<String>, ApplicationError> {
    let pattern = format!("{prefix}%");
    sqlx::query_scalar(
        "SELECT stream.stream_key
         FROM public.realtime_streams AS stream
         LEFT JOIN public.realtime_consumer_offsets AS offset_row
           ON offset_row.consumer_key = $1 AND offset_row.stream_key = stream.stream_key
         WHERE stream.stream_key LIKE $2 AND stream.stream_key > $3
           AND stream.next_sequence - 1 > COALESCE(offset_row.last_sequence, 0)
         ORDER BY stream.stream_key ASC
         LIMIT $4",
    )
    .bind(consumer_key)
    .bind(pattern)
    .bind(after)
    .bind(i64::from(limit))
    .fetch_all(connection)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 实时 stream 目录失败"))
}
