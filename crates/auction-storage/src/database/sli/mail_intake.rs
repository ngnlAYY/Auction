//! 入站邮件到成功出站证明的历史窗口：注册 key、投稿 Take、拒收通知。

use auction_application::{
    SLI_CANDIDATE_LIMIT, SLI_WINDOW_SECONDS, SliFlowWindow, SliSnapshotError, SliWorld,
};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};

use super::{candidate_limit, storage};

const REGISTER_COUNT: &str = "
    WITH candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.origin_kind='mail_intake'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
          AND delivery.payload->>'operation'='send'
          AND delivery.payload->>'template'='register_key'
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    )
    SELECT count(*) AS candidates,
           count(*) FILTER (WHERE inbound.id IS NULL) AS invalid
    FROM candidates candidate
    LEFT JOIN public.inbound_mail inbound
      ON inbound.world_id=candidate.payload->>'world'
     AND inbound.external_mail_id=candidate.payload->>'external_mail_id'";

const REGISTER_AGGREGATE: &str = "
    WITH candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.origin_kind='mail_intake'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
          AND delivery.payload->>'operation'='send'
          AND delivery.payload->>'template'='register_key'
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    )
    SELECT inbound.world_id,count(*) AS samples,
           percentile_cont(0.5) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-inbound.received_at))
           ) AS p50,
           percentile_cont(0.95) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-inbound.received_at))
           ) AS p95,
           percentile_cont(0.99) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-inbound.received_at))
           ) AS p99
    FROM candidates candidate
    JOIN public.inbound_mail inbound
      ON inbound.world_id=candidate.payload->>'world'
     AND inbound.external_mail_id=candidate.payload->>'external_mail_id'
    GROUP BY inbound.world_id";

const TAKE_COUNT: &str = "
    WITH candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.origin_kind='mail_intake'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
          AND delivery.payload->>'operation'='take'
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    )
    SELECT count(*) AS candidates,
           count(*) FILTER (WHERE inbound.id IS NULL) AS invalid
    FROM candidates candidate
    LEFT JOIN public.inbound_mail inbound
      ON inbound.world_id=candidate.payload->>'world'
     AND inbound.external_mail_id=candidate.payload->>'external_mail_id'";

const TAKE_AGGREGATE: &str = "
    WITH candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.origin_kind='mail_intake'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
          AND delivery.payload->>'operation'='take'
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    )
    SELECT inbound.world_id,count(*) AS samples,
           percentile_cont(0.5) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-inbound.received_at))
           ) AS p50,
           percentile_cont(0.95) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-inbound.received_at))
           ) AS p95,
           percentile_cont(0.99) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-inbound.received_at))
           ) AS p99
    FROM candidates candidate
    JOIN public.inbound_mail inbound
      ON inbound.world_id=candidate.payload->>'world'
     AND inbound.external_mail_id=candidate.payload->>'external_mail_id'
    GROUP BY inbound.world_id";

const REJECTION_COUNT: &str = "
    WITH candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.origin_kind='mail_intake'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
          AND delivery.payload->>'operation'='send'
          AND delivery.payload->>'template' IN
              ('auction_equipment_rejected','auction_no_attachments')
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    )
    SELECT count(*) AS candidates,
           count(*) FILTER (WHERE inbound.id IS NULL) AS invalid
    FROM candidates candidate
    LEFT JOIN public.inbound_mail inbound
      ON inbound.world_id=candidate.payload->>'world'
     AND inbound.external_mail_id=candidate.payload->>'external_mail_id'";

const REJECTION_AGGREGATE: &str = "
    WITH candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.origin_kind='mail_intake'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
          AND delivery.payload->>'operation'='send'
          AND delivery.payload->>'template' IN
              ('auction_equipment_rejected','auction_no_attachments')
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    )
    SELECT inbound.world_id,count(*) AS samples,
           percentile_cont(0.5) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-inbound.received_at))
           ) AS p50,
           percentile_cont(0.95) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-inbound.received_at))
           ) AS p95,
           percentile_cont(0.99) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-inbound.received_at))
           ) AS p99
    FROM candidates candidate
    JOIN public.inbound_mail inbound
      ON inbound.world_id=candidate.payload->>'world'
     AND inbound.external_mail_id=candidate.payload->>'external_mail_id'
    GROUP BY inbound.world_id";

async fn read_flow(
    connection: &mut PgConnection,
    now: DateTime<Utc>,
    count_sql: &'static str,
    aggregate_sql: &'static str,
) -> Result<[SliFlowWindow; SliWorld::ALL.len()], SliSnapshotError> {
    let since = now
        .checked_sub_signed(chrono::Duration::seconds(SLI_WINDOW_SECONDS))
        .ok_or(SliSnapshotError::InvalidData)?;
    let row = sqlx::query(count_sql)
        .bind(since)
        .bind(now)
        .bind(candidate_limit())
        .fetch_one(&mut *connection)
        .await
        .map_err(storage)?;
    let candidate_count: i64 = row.try_get("candidates").map_err(storage)?;
    let invalid: i64 = row.try_get("invalid").map_err(storage)?;
    if candidate_count > SLI_CANDIDATE_LIMIT as i64 {
        return Err(SliSnapshotError::Limited);
    }
    if invalid > 0 {
        return Err(SliSnapshotError::InvalidData);
    }
    let rows = sqlx::query(aggregate_sql)
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

pub(super) async fn register_key(
    connection: &mut PgConnection,
    now: DateTime<Utc>,
) -> Result<[SliFlowWindow; SliWorld::ALL.len()], SliSnapshotError> {
    read_flow(connection, now, REGISTER_COUNT, REGISTER_AGGREGATE).await
}

pub(super) async fn submission_take(
    connection: &mut PgConnection,
    now: DateTime<Utc>,
) -> Result<[SliFlowWindow; SliWorld::ALL.len()], SliSnapshotError> {
    read_flow(connection, now, TAKE_COUNT, TAKE_AGGREGATE).await
}

pub(super) async fn submission_rejection(
    connection: &mut PgConnection,
    now: DateTime<Utc>,
) -> Result<[SliFlowWindow; SliWorld::ALL.len()], SliSnapshotError> {
    read_flow(connection, now, REJECTION_COUNT, REJECTION_AGGREGATE).await
}
