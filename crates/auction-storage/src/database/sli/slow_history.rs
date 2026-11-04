//! 慢速代理历史执行滞后：只读审计的 due_at → occurred_at 窗口。

use auction_application::{
    SLI_CANDIDATE_LIMIT, SLI_WINDOW_SECONDS, SliFlowWindow, SliSnapshotError, SliWorld,
};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};

use super::{candidate_limit, storage};

const SLOW_COUNT: &str = "
    WITH candidates AS MATERIALIZED (
        SELECT audit.id,audit.occurred_at,audit.entity_id,audit.metadata,
               CASE WHEN audit.metadata->>'due_at' ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}T'
                    THEN (audit.metadata->>'due_at')::timestamptz END AS due_at,
               CASE WHEN audit.entity_id ~
                         '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
                    THEN audit.entity_id::uuid END AS lot_id
        FROM public.audit_records audit
        WHERE audit.action='lot.slow_proxy_tick'
          AND audit.occurred_at >= $1 AND audit.occurred_at <= $2
        ORDER BY audit.occurred_at,audit.id
        LIMIT $3
    )
    SELECT count(*) AS candidates,
           count(*) FILTER (WHERE due_at IS NULL OR lot_id IS NULL) AS invalid
    FROM candidates";

const SLOW_AGGREGATE: &str = "
    WITH candidates AS MATERIALIZED (
        SELECT audit.id,audit.occurred_at,audit.entity_id,audit.metadata,
               CASE WHEN audit.metadata->>'due_at' ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}T'
                    THEN (audit.metadata->>'due_at')::timestamptz END AS due_at,
               CASE WHEN audit.entity_id ~
                         '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
                    THEN audit.entity_id::uuid END AS lot_id
        FROM public.audit_records audit
        WHERE audit.action='lot.slow_proxy_tick'
          AND audit.occurred_at >= $1 AND audit.occurred_at <= $2
        ORDER BY audit.occurred_at,audit.id
        LIMIT $3
    )
    SELECT auction.world_id,count(*) AS samples,
           percentile_cont(0.5) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.occurred_at-candidate.due_at))
           ) AS p50,
           percentile_cont(0.95) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.occurred_at-candidate.due_at))
           ) AS p95,
           percentile_cont(0.99) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.occurred_at-candidate.due_at))
           ) AS p99
    FROM candidates candidate
    JOIN public.lots lot ON lot.id=candidate.lot_id
    JOIN public.auctions auction ON auction.id=lot.auction_id
    WHERE candidate.due_at IS NOT NULL
    GROUP BY auction.world_id";

pub(super) async fn read(
    connection: &mut PgConnection,
    now: DateTime<Utc>,
) -> Result<[SliFlowWindow; SliWorld::ALL.len()], SliSnapshotError> {
    let since = now
        .checked_sub_signed(chrono::Duration::seconds(SLI_WINDOW_SECONDS))
        .ok_or(SliSnapshotError::InvalidData)?;
    let row = sqlx::query(SLOW_COUNT)
        .bind(since)
        .bind(now)
        .bind(candidate_limit())
        .fetch_one(&mut *connection)
        .await
        .map_err(storage)?;
    let candidates: i64 = row.try_get("candidates").map_err(storage)?;
    let invalid: i64 = row.try_get("invalid").map_err(storage)?;
    if candidates > SLI_CANDIDATE_LIMIT as i64 {
        return Err(SliSnapshotError::Limited);
    }
    if invalid > 0 {
        return Err(SliSnapshotError::InvalidData);
    }
    let rows = sqlx::query(SLOW_AGGREGATE)
        .bind(since)
        .bind(now)
        .bind(candidate_limit())
        .fetch_all(&mut *connection)
        .await
        .map_err(storage)?;
    let mut windows = [SliFlowWindow::default(); SliWorld::ALL.len()];
    for row in rows {
        let world: String = row.try_get("world_id").map_err(storage)?;
        let index = match world.as_str() {
            "main" => 0,
            "isekai" => 1,
            _ => return Err(SliSnapshotError::InvalidData),
        };
        let samples: i64 = row.try_get("samples").map_err(storage)?;
        windows[index] = SliFlowWindow {
            samples: u64::try_from(samples).map_err(|error| {
                let _ = error;
                SliSnapshotError::InvalidData
            })?,
            p50: row.try_get("p50").map_err(storage)?,
            p95: row.try_get("p95").map_err(storage)?,
            p99: row.try_get("p99").map_err(storage)?,
            unknown: 0,
            incomplete: 0,
            oldest_incomplete_seconds: None,
        };
    }
    Ok(windows)
}
