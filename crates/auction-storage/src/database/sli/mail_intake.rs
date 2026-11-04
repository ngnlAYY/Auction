//! 入站邮件到成功出站证明的历史窗口：注册 key、投稿 Take、拒收通知。
//! Send 复用 delivery_has_frozen_sent_proof；Take 复用 conflicts/intake 的 owner 谓词。
//! 证明缺失或不符合谓词的已成功投递计入 unknown，不进入分位。

use auction_application::{
    SLI_CANDIDATE_LIMIT, SLI_WINDOW_SECONDS, SliFlowWindow, SliSnapshotError, SliWorld,
};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};

use super::{candidate_limit, storage};

const REGISTER_COUNT: &str = "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.origin_kind,
               delivery.status,delivery.failure_code,delivery.lease_token,
               delivery.lease_expires_at,delivery.sending_at,
               delivery.result_metadata,delivery.external_reference
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    ),
    candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.origin_kind,
               delivery.result_metadata,delivery.external_reference
        FROM window_deliveries delivery
        WHERE delivery.status='succeeded' AND delivery.origin_kind='mail_intake'
          AND delivery.payload->>'operation'='send'
          AND delivery.payload->>'template'='register_key'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    )
    SELECT (SELECT count(*) FROM window_deliveries) AS window_count,
           count(*) AS candidates,
           count(*) FILTER (WHERE inbound.id IS NULL) AS invalid
    FROM candidates candidate
    LEFT JOIN public.inbound_mail inbound
      ON inbound.world_id=candidate.payload->>'world'
     AND inbound.external_mail_id=candidate.payload->>'external_mail_id'";

const REGISTER_UNKNOWN: &str = "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.origin_kind,
               delivery.status,delivery.failure_code,delivery.lease_token,
               delivery.lease_expires_at,delivery.sending_at,
               delivery.result_metadata,delivery.external_reference
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    ),
    candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.origin_kind,
               delivery.result_metadata,delivery.external_reference
        FROM window_deliveries delivery
        WHERE delivery.status='succeeded' AND delivery.origin_kind='mail_intake'
          AND delivery.payload->>'operation'='send'
          AND delivery.payload->>'template'='register_key'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    )
    SELECT inbound.world_id,count(*) AS unknown
    FROM candidates candidate
    JOIN public.inbound_mail inbound
      ON inbound.world_id=candidate.payload->>'world'
     AND inbound.external_mail_id=candidate.payload->>'external_mail_id'
    WHERE NOT public.delivery_has_frozen_sent_proof(
        candidate.id,candidate.result_metadata,candidate.external_reference
    )
    GROUP BY inbound.world_id";

const REGISTER_AGGREGATE: &str = "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.origin_kind,
               delivery.status,delivery.failure_code,delivery.lease_token,
               delivery.lease_expires_at,delivery.sending_at,
               delivery.result_metadata,delivery.external_reference
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    ),
    candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.origin_kind,
               delivery.result_metadata,delivery.external_reference
        FROM window_deliveries delivery
        WHERE delivery.status='succeeded' AND delivery.origin_kind='mail_intake'
          AND delivery.payload->>'operation'='send'
          AND delivery.payload->>'template'='register_key'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
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
    WHERE public.delivery_has_frozen_sent_proof(
        candidate.id,candidate.result_metadata,candidate.external_reference
    )
    GROUP BY inbound.world_id";

const TAKE_COUNT: &str = "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.origin_kind,
               delivery.status,delivery.failure_code,delivery.lease_token,
               delivery.lease_expires_at,delivery.sending_at,
               delivery.result_metadata,delivery.external_reference
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    ),
    candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload
        FROM window_deliveries delivery
        WHERE delivery.status='succeeded' AND delivery.origin_kind='mail_intake'
          AND delivery.payload->>'operation'='take'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    )
    SELECT (SELECT count(*) FROM window_deliveries) AS window_count,
           count(*) AS candidates,
           count(*) FILTER (WHERE inbound.id IS NULL) AS invalid
    FROM candidates candidate
    LEFT JOIN public.inbound_mail inbound
      ON inbound.world_id=candidate.payload->>'world'
     AND inbound.external_mail_id=candidate.payload->>'external_mail_id'";

const TAKE_UNKNOWN: &str = "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.origin_kind,
               delivery.status,delivery.failure_code,delivery.lease_token,
               delivery.lease_expires_at,delivery.sending_at,
               delivery.result_metadata,delivery.external_reference
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    ),
    candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,
               (delivery.failure_code IS NULL AND delivery.lease_token IS NULL
                AND delivery.lease_expires_at IS NULL AND delivery.sending_at IS NOT NULL
                AND delivery.completed_at >= delivery.sending_at
                AND delivery.external_reference =
                    'mooglemail:'||(delivery.payload->>'world')||':take:'
                    ||(delivery.payload->>'external_mail_id')
                AND (
                    (delivery.result_metadata->>'proof'='action_removed_or_success_page'
                     AND delivery.result_metadata->>'operation'='take'
                     AND delivery.result_metadata->>'external_mail_id'=
                         delivery.payload->>'external_mail_id'
                     AND COALESCE(delivery.result_metadata->>'outcome_unknown','false')='false'
                     AND delivery.result_metadata->>'expected_credits'='0'
                     AND delivery.result_metadata->>'expected_cod'='0')
                    OR
                    (delivery.result_metadata->>'proof'='action_recovered_inbox_v1'
                     AND delivery.result_metadata->>'external_reference_proven'='true'
                     AND delivery.result_metadata->>'recovered_mid'=
                         delivery.payload->>'external_mail_id'
                     AND delivery.result_metadata->>'reason' IN
                         ('inbox_mail_absent','attachments_absent'))
                )) AS proof_ok
        FROM window_deliveries delivery
        WHERE delivery.status='succeeded' AND delivery.origin_kind='mail_intake'
          AND delivery.payload->>'operation'='take'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    )
    SELECT inbound.world_id,count(*) AS unknown
    FROM candidates candidate
    JOIN public.inbound_mail inbound
      ON inbound.world_id=candidate.payload->>'world'
     AND inbound.external_mail_id=candidate.payload->>'external_mail_id'
    WHERE candidate.proof_ok IS NOT TRUE
    GROUP BY inbound.world_id";

const TAKE_AGGREGATE: &str = "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.origin_kind,
               delivery.status,delivery.failure_code,delivery.lease_token,
               delivery.lease_expires_at,delivery.sending_at,
               delivery.result_metadata,delivery.external_reference
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    ),
    candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,
               (delivery.failure_code IS NULL AND delivery.lease_token IS NULL
                AND delivery.lease_expires_at IS NULL AND delivery.sending_at IS NOT NULL
                AND delivery.completed_at >= delivery.sending_at
                AND delivery.external_reference =
                    'mooglemail:'||(delivery.payload->>'world')||':take:'
                    ||(delivery.payload->>'external_mail_id')
                AND (
                    (delivery.result_metadata->>'proof'='action_removed_or_success_page'
                     AND delivery.result_metadata->>'operation'='take'
                     AND delivery.result_metadata->>'external_mail_id'=
                         delivery.payload->>'external_mail_id'
                     AND COALESCE(delivery.result_metadata->>'outcome_unknown','false')='false'
                     AND delivery.result_metadata->>'expected_credits'='0'
                     AND delivery.result_metadata->>'expected_cod'='0')
                    OR
                    (delivery.result_metadata->>'proof'='action_recovered_inbox_v1'
                     AND delivery.result_metadata->>'external_reference_proven'='true'
                     AND delivery.result_metadata->>'recovered_mid'=
                         delivery.payload->>'external_mail_id'
                     AND delivery.result_metadata->>'reason' IN
                         ('inbox_mail_absent','attachments_absent'))
                )) AS proof_ok
        FROM window_deliveries delivery
        WHERE delivery.status='succeeded' AND delivery.origin_kind='mail_intake'
          AND delivery.payload->>'operation'='take'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
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
    WHERE candidate.proof_ok
    GROUP BY inbound.world_id";

const REJECTION_COUNT: &str = "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.origin_kind,
               delivery.status,delivery.failure_code,delivery.lease_token,
               delivery.lease_expires_at,delivery.sending_at,
               delivery.result_metadata,delivery.external_reference
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    ),
    candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.origin_kind,
               delivery.result_metadata,delivery.external_reference
        FROM window_deliveries delivery
        WHERE delivery.status='succeeded' AND delivery.origin_kind='mail_intake'
          AND delivery.payload->>'operation'='send'
          AND delivery.payload->>'template' IN
              ('auction_equipment_rejected','auction_no_attachments','auction_assets_rejected')
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    )
    SELECT (SELECT count(*) FROM window_deliveries) AS window_count,
           count(*) AS candidates,
           count(*) FILTER (WHERE inbound.id IS NULL) AS invalid
    FROM candidates candidate
    LEFT JOIN public.inbound_mail inbound
      ON inbound.world_id=candidate.payload->>'world'
     AND inbound.external_mail_id=candidate.payload->>'external_mail_id'";

const REJECTION_UNKNOWN: &str = "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.origin_kind,
               delivery.status,delivery.failure_code,delivery.lease_token,
               delivery.lease_expires_at,delivery.sending_at,
               delivery.result_metadata,delivery.external_reference
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    ),
    candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.origin_kind,
               delivery.result_metadata,delivery.external_reference
        FROM window_deliveries delivery
        WHERE delivery.status='succeeded' AND delivery.origin_kind='mail_intake'
          AND delivery.payload->>'operation'='send'
          AND delivery.payload->>'template' IN
              ('auction_equipment_rejected','auction_no_attachments','auction_assets_rejected')
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    )
    SELECT inbound.world_id,count(*) AS unknown
    FROM candidates candidate
    JOIN public.inbound_mail inbound
      ON inbound.world_id=candidate.payload->>'world'
     AND inbound.external_mail_id=candidate.payload->>'external_mail_id'
    WHERE NOT public.delivery_has_frozen_sent_proof(
        candidate.id,candidate.result_metadata,candidate.external_reference
    )
    GROUP BY inbound.world_id";

const REJECTION_AGGREGATE: &str = "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.origin_kind,
               delivery.status,delivery.failure_code,delivery.lease_token,
               delivery.lease_expires_at,delivery.sending_at,
               delivery.result_metadata,delivery.external_reference
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    ),
    candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.origin_kind,
               delivery.result_metadata,delivery.external_reference
        FROM window_deliveries delivery
        WHERE delivery.status='succeeded' AND delivery.origin_kind='mail_intake'
          AND delivery.payload->>'operation'='send'
          AND delivery.payload->>'template' IN
              ('auction_equipment_rejected','auction_no_attachments','auction_assets_rejected')
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
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
    WHERE public.delivery_has_frozen_sent_proof(
        candidate.id,candidate.result_metadata,candidate.external_reference
    )
    GROUP BY inbound.world_id";

async fn read_flow(
    connection: &mut PgConnection,
    now: DateTime<Utc>,
    count_sql: &'static str,
    unknown_sql: &'static str,
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
    let window_count: i64 = row.try_get("window_count").map_err(storage)?;
    let candidate_count: i64 = row.try_get("candidates").map_err(storage)?;
    let invalid: i64 = row.try_get("invalid").map_err(storage)?;
    if window_count > SLI_CANDIDATE_LIMIT as i64 || candidate_count > SLI_CANDIDATE_LIMIT as i64 {
        return Err(SliSnapshotError::Limited);
    }
    if invalid > 0 {
        return Err(SliSnapshotError::InvalidData);
    }
    let mut windows = [SliFlowWindow::default(); SliWorld::ALL.len()];
    for row in sqlx::query(unknown_sql)
        .bind(since)
        .bind(now)
        .bind(candidate_limit())
        .fetch_all(&mut *connection)
        .await
        .map_err(storage)?
    {
        let world: String = row.try_get("world_id").map_err(storage)?;
        let index = match world.as_str() {
            "main" => 0,
            "isekai" => 1,
            _ => return Err(SliSnapshotError::InvalidData),
        };
        let unknown: i64 = row.try_get("unknown").map_err(storage)?;
        windows[index].unknown = u64::try_from(unknown).map_err(|error| {
            let _ = error;
            SliSnapshotError::InvalidData
        })?;
    }
    for row in sqlx::query(aggregate_sql)
        .bind(since)
        .bind(now)
        .bind(candidate_limit())
        .fetch_all(&mut *connection)
        .await
        .map_err(storage)?
    {
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
            unknown: windows[index].unknown,
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
    read_flow(connection, now, REGISTER_COUNT, REGISTER_UNKNOWN, REGISTER_AGGREGATE).await
}

pub(super) async fn submission_take(
    connection: &mut PgConnection,
    now: DateTime<Utc>,
) -> Result<[SliFlowWindow; SliWorld::ALL.len()], SliSnapshotError> {
    read_flow(connection, now, TAKE_COUNT, TAKE_UNKNOWN, TAKE_AGGREGATE).await
}

pub(super) async fn submission_rejection(
    connection: &mut PgConnection,
    now: DateTime<Utc>,
) -> Result<[SliFlowWindow; SliWorld::ALL.len()], SliSnapshotError> {
    read_flow(connection, now, REJECTION_COUNT, REJECTION_UNKNOWN, REJECTION_AGGREGATE).await
}
