//! Realtime stream 目录查询适配器。

use auction_application::core::ApplicationError;

use crate::database::storage_error;
use crate::database::storage_error_from;
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
///
/// 迁移 000011 建立的 `auction:discovery` 历史发现流只保留、不再写入、没有读者：目录不列出它，
/// 新 consumer（缺失游标按 0）也不会读取保留期内的历史事件并把它们记成失败。
pub(super) async fn list_pending_streams_postgres(
    connection: &mut sqlx::PgConnection,
    consumer_key: &str,
    prefix: &str,
    after: &str,
    limit: u16,
) -> Result<Vec<String>, ApplicationError> {
    let pattern = format!("{prefix}%");
    sqlx::query_scalar(concat!(
        "SELECT stream.stream_key
         FROM public.realtime_streams AS stream
         LEFT JOIN public.realtime_consumer_offsets AS offset_row
           ON offset_row.consumer_key = $1 AND offset_row.stream_key = stream.stream_key
         WHERE stream.stream_key LIKE $2 AND stream.stream_key > $3
           AND ",
        crate::database::realtime::discovery_stream_excluded_sql!(),
        "
           AND stream.next_sequence - 1 > COALESCE(offset_row.last_sequence, 0)
         ORDER BY stream.stream_key ASC
         LIMIT $4"
    ))
    .bind(consumer_key)
    .bind(pattern)
    .bind(after)
    .bind(i64::from(limit))
    .fetch_all(connection)
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL 实时 stream 目录失败"))
}
