//! realtime stream、consumer 和 failure 水位盘点。

use super::mapping::*;
use super::*;
use sqlx::{Row, postgres::PgRow};

pub(super) async fn read_realtime_inventory(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<PostgresRealtimeInventory, RetentionInventoryError> {
    let rows = sqlx::query(
        "WITH selected_streams AS (
             SELECT stream_key
             FROM public.realtime_streams
             ORDER BY stream_key ASC
             LIMIT $1
         ), event_summary AS (
             SELECT event.stream_key,
                    MIN(event.stream_sequence)::bigint AS min_sequence,
                    MAX(event.stream_sequence)::bigint AS max_sequence,
                    COUNT(*)::bigint AS event_count
             FROM public.realtime_events AS event
             JOIN selected_streams AS selected ON selected.stream_key = event.stream_key
             GROUP BY event.stream_key
         ), offset_summary AS (
             SELECT offset_row.stream_key,
                    MIN(offset_row.last_sequence)::bigint AS diagnostic_min_consumer_offset,
                    COUNT(*)::bigint AS consumer_count
             FROM public.realtime_consumer_offsets AS offset_row
             JOIN selected_streams AS selected ON selected.stream_key = offset_row.stream_key
             GROUP BY offset_row.stream_key
         ), failure_summary AS (
             SELECT event.stream_key,
                    MIN(event.stream_sequence) FILTER (WHERE failure.resolved_at IS NULL)::bigint
                        AS unresolved_failure_min_sequence,
                    MAX(event.stream_sequence) FILTER (WHERE failure.resolved_at IS NULL)::bigint
                        AS unresolved_failure_max_sequence
             FROM public.realtime_failures AS failure
             JOIN public.realtime_events AS event ON event.event_id = failure.event_id
             JOIN selected_streams AS selected ON selected.stream_key = event.stream_key
             GROUP BY event.stream_key
         )
         SELECT selected.stream_key,
                event.min_sequence,
                event.max_sequence,
                COALESCE(event.event_count, 0)::bigint AS event_count,
                COALESCE(offsets.consumer_count, 0)::bigint AS consumer_count,
                offsets.diagnostic_min_consumer_offset,
                COALESCE((
                    SELECT COUNT(*)::bigint
                    FROM public.realtime_events AS candidate
                    WHERE candidate.stream_key = selected.stream_key
                      AND offsets.diagnostic_min_consumer_offset IS NOT NULL
                      AND candidate.stream_sequence <= offsets.diagnostic_min_consumer_offset
                ), 0)::bigint AS events_below_diagnostic_min_offset,
                failures.unresolved_failure_min_sequence,
                failures.unresolved_failure_max_sequence
         FROM selected_streams AS selected
         LEFT JOIN event_summary AS event ON event.stream_key = selected.stream_key
         LEFT JOIN offset_summary AS offsets ON offsets.stream_key = selected.stream_key
         LEFT JOIN failure_summary AS failures ON failures.stream_key = selected.stream_key
         ORDER BY selected.stream_key ASC",
    )
    .bind(REALTIME_STREAM_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|source| query_error("realtime_stream_inventory", source))?;
    let streams = rows.into_iter().map(map_realtime_stream).collect::<Result<Vec<_>, _>>()?;

    let coverage_row = sqlx::query(
        "SELECT
             (SELECT COUNT(*)::bigint FROM public.realtime_streams) AS configured_stream_count,
             (SELECT COUNT(*)::bigint FROM public.realtime_consumer_offsets)
                 AS configured_consumer_offset_count",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("realtime_coverage", source))?;
    let configured_stream_count: i64 = coverage_row
        .try_get("configured_stream_count")
        .map_err(|source| query_error("realtime_coverage", source))?;
    let configured_consumer_offset_count: i64 = coverage_row
        .try_get("configured_consumer_offset_count")
        .map_err(|source| query_error("realtime_coverage", source))?;
    let diagnostic_consumer_offset_count = streams
        .iter()
        .try_fold(0_i64, |total, stream| total.checked_add(stream.consumer_count))
        .ok_or(RetentionInventoryError::CountOverflow)?;
    let reported_stream_count =
        i64::try_from(streams.len()).map_err(|_| RetentionInventoryError::CountOverflow)?;

    Ok(PostgresRealtimeInventory {
        streams,
        coverage: PostgresRealtimeCoverage {
            configured_stream_count,
            reported_stream_count,
            stream_rows_truncated: configured_stream_count > MAX_REALTIME_STREAMS,
            configured_consumer_offset_count,
            diagnostic_consumer_offset_count,
            eligibility_assessed: false,
            eligibility_authorized: false,
        },
    })
}

pub(super) fn map_realtime_stream(
    row: PgRow,
) -> Result<PostgresRealtimeStreamInventory, RetentionInventoryError> {
    Ok(PostgresRealtimeStreamInventory {
        stream_key: row
            .try_get("stream_key")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        min_sequence: row
            .try_get("min_sequence")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        max_sequence: row
            .try_get("max_sequence")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        event_count: row
            .try_get("event_count")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        consumer_count: row
            .try_get("consumer_count")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        diagnostic_min_consumer_offset: row
            .try_get("diagnostic_min_consumer_offset")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        events_below_diagnostic_min_offset: row
            .try_get("events_below_diagnostic_min_offset")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        unresolved_failure_min_sequence: row
            .try_get("unresolved_failure_min_sequence")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        unresolved_failure_max_sequence: row
            .try_get("unresolved_failure_max_sequence")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        watermark_kind: "all_configured_consumers_not_authoritative_safe_watermark",
        eligibility_assessed: false,
        eligibility_authorized: false,
    })
}
