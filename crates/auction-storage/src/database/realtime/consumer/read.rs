//! Realtime 事件批量读取与公共事件安全映射。

use auction_application::core::ApplicationError;
use auction_application::realtime::{
    PublicRealtimeEvent, RealtimeConsumerBatch, RealtimeConsumerRequest,
};
use sqlx::Connection;

use crate::database::{realtime::event_from_postgres_row, storage_error_from};

pub(super) async fn read_postgres(
    connection: &mut sqlx::PgConnection,
    request: RealtimeConsumerRequest,
) -> Result<Option<RealtimeConsumerBatch>, ApplicationError> {
    let mut transaction = connection
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "打开 PostgreSQL 实时消费快照失败"))?;
    let stream_exists: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM public.realtime_streams WHERE stream_key = $1")
            .bind(&request.stream_key)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 实时 stream 目录失败"))?;
    if stream_exists.is_none() {
        transaction
            .commit()
            .await
            .map_err(|error| storage_error_from(error, "提交 PostgreSQL 实时 stream 快照失败"))?;
        return Ok(None);
    }

    let after = sqlx::query_scalar::<_, i64>(
        "SELECT last_sequence
         FROM public.realtime_consumer_offsets
         WHERE consumer_key = $1 AND stream_key = $2",
    )
    .bind(&request.consumer_key)
    .bind(&request.stream_key)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL 实时消费游标失败"))?
    .unwrap_or(0);
    let rows = sqlx::query(
        "SELECT event_id, stream_key, stream_sequence, kind, schema_version,
                public_payload, created_at
         FROM public.realtime_events
         WHERE stream_key = $1 AND stream_sequence > $2
         ORDER BY stream_sequence ASC
         LIMIT $3",
    )
    .bind(&request.stream_key)
    .bind(after)
    .bind(i64::from(request.batch_size))
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL 实时消费事件失败"))?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交 PostgreSQL 实时消费快照失败"))?;

    let events = rows.iter().map(event_from_postgres_row).collect::<Result<Vec<_>, _>>()?;
    Ok(Some(batch(request, after, events)))
}

fn batch(
    request: RealtimeConsumerRequest,
    after: i64,
    events: Vec<PublicRealtimeEvent>,
) -> RealtimeConsumerBatch {
    let sequence_gap = events.first().is_some_and(|event| {
        after.checked_add(1).is_none_or(|next_sequence| event.stream_sequence > next_sequence)
            || event.stream_sequence <= after
            || event.stream_key != request.stream_key
    });
    RealtimeConsumerBatch {
        consumer_key: request.consumer_key,
        stream_key: request.stream_key,
        after,
        events,
        sequence_gap,
    }
}
