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

#[cfg(feature = "postgres")]
pub(super) async fn list_streams_postgres(
    connection: &mut sqlx::PgConnection,
    prefix: &str,
    after: &str,
    limit: u16,
) -> Result<Vec<String>, ApplicationError> {
    let pattern = format!("{prefix}%");
    sqlx::query_scalar(
        "SELECT stream_key
         FROM public.realtime_streams
         WHERE stream_key LIKE $1 AND stream_key > $2
         ORDER BY stream_key ASC
         LIMIT $3",
    )
    .bind(pattern)
    .bind(after)
    .bind(i64::from(limit))
    .fetch_all(connection)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 实时 stream 目录失败"))
}
