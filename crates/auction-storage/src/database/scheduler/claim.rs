//! Worker 调度 due claim 与租约领取。

use auction_application::scheduling::ScheduledRoundClaim;
use auction_domain::AuctionId;
use chrono::{DateTime, NaiveDate, Utc};
#[cfg(feature = "postgres")]
use uuid::Uuid;

#[cfg(feature = "postgres")]
use sqlx::Postgres;
#[cfg(feature = "postgres")]
use sqlx::Row;

use super::mapping::{
    command_id, map_storage_error, phase_from_completion, repository_error_owned, round_identity,
};
use crate::database::TrackedPool;

#[cfg(feature = "postgres")]
pub(super) async fn postgres(
    pool: &TrackedPool<Postgres>,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: u16,
) -> Result<Vec<ScheduledRoundClaim>, auction_application::core::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    // Preview/Open 不消费 lot 截止；只为已 Open 的 Live 轮次执行 Final 到期聚合。
    let rows = sqlx::query(
        "SELECT round.id, round.auction_id, round.create_command_id,
                round.world_id, round.local_week, round.slot_key,
                round.preview_completed_at, round.open_completed_at, round.attempt_count,
                round.state_version
         FROM public.scheduled_rounds AS round
         INNER JOIN public.auctions AS auction ON auction.id = round.auction_id
         LEFT JOIN LATERAL (
             SELECT MIN(lot.effective_end_at) AS earliest_end_at
             FROM public.lots AS lot
             WHERE lot.auction_id = round.auction_id AND lot.status = 'live'
               AND round.open_completed_at IS NOT NULL AND auction.status = 'live'
         ) AS deadline ON true
         WHERE round.status = 'active'
           AND round.final_completed_at IS NULL
           AND round.last_failure_code IS DISTINCT FROM 'task_panic_terminal'
           AND (round.lease_until IS NULL OR round.lease_until <= $1)
           AND (
               (round.preview_completed_at IS NULL AND round.preview_due_at <= $1)
               OR (
                   round.preview_completed_at IS NOT NULL
                   AND round.open_completed_at IS NULL
                   AND round.open_due_at <= $1
               )
               OR (
                   round.open_completed_at IS NOT NULL
                   AND auction.status = 'live'
                   AND deadline.earliest_end_at IS NOT NULL
                   AND deadline.earliest_end_at
                       - ((auction.policy_snapshot ->> 'final_phase_lead_seconds')::double precision
                          * interval '1 second') <= $1
               )
           )
         ORDER BY CASE
             WHEN round.preview_completed_at IS NULL THEN round.preview_due_at
             WHEN round.open_completed_at IS NULL THEN round.open_due_at
             ELSE deadline.earliest_end_at
                 - ((auction.policy_snapshot ->> 'final_phase_lead_seconds')::double precision
                    * interval '1 second')
         END, round.id
         LIMIT $2
         FOR UPDATE OF round SKIP LOCKED",
    )
    .bind(now)
    .bind(i64::from(limit))
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    let mut claims = Vec::with_capacity(rows.len());
    for row in rows {
        let round_id: Uuid = row.try_get("id").map_err(map_storage_error)?;
        let auction_id =
            AuctionId::from_uuid(row.try_get("auction_id").map_err(map_storage_error)?);
        let create_command_id: Uuid =
            row.try_get("create_command_id").map_err(map_storage_error)?;
        let world_id: String = row.try_get("world_id").map_err(map_storage_error)?;
        let local_week: NaiveDate = row.try_get("local_week").map_err(map_storage_error)?;
        let slot_key: Option<String> = row.try_get("slot_key").map_err(map_storage_error)?;
        let preview_completed_at: Option<DateTime<Utc>> =
            row.try_get("preview_completed_at").map_err(map_storage_error)?;
        let open_completed_at: Option<DateTime<Utc>> =
            row.try_get("open_completed_at").map_err(map_storage_error)?;
        let phase =
            phase_from_completion(preview_completed_at.is_some(), open_completed_at.is_some());
        let old_attempt_count: i32 = row.try_get("attempt_count").map_err(map_storage_error)?;
        let attempt_count = old_attempt_count
            .checked_add(1)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| repository_error_owned("Worker 尝试次数溢出"))?;
        let old_state_version: i64 = row.try_get("state_version").map_err(map_storage_error)?;
        let updated = sqlx::query(
            "UPDATE public.scheduled_rounds
             SET lease_owner = $2, lease_until = $3, attempt_count = $4,
                 state_version = state_version + 1,
                 last_failure_code = NULL, last_failure_detail = NULL,
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = $1 AND status = 'active' AND final_completed_at IS NULL
               AND state_version = $5
             RETURNING state_version",
        )
        .bind(round_id)
        .bind(worker_id)
        .bind(lease_until)
        .bind(
            i32::try_from(attempt_count)
                .map_err(|_| repository_error_owned("Worker 尝试次数超出数据库范围"))?,
        )
        .bind(old_state_version)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_storage_error)?;
        let Some(updated) = updated else {
            return Err(repository_error_owned("Worker 领取轮次时状态已变化"));
        };
        let state_version: i64 = updated.try_get("state_version").map_err(map_storage_error)?;
        let expected_state_version = old_state_version
            .checked_add(1)
            .ok_or_else(|| repository_error_owned("Worker 调度版本溢出"))?;
        if state_version != expected_state_version {
            return Err(repository_error_owned("Worker 领取轮次后的版本无效"));
        }
        claims.push(ScheduledRoundClaim {
            round_id,
            auction_id,
            phase,
            command_id: command_id(
                create_command_id,
                &world_id,
                &round_identity(local_week, slot_key.as_deref()),
                phase,
            ),
            worker_id: worker_id.to_owned(),
            lease_until,
            state_version,
            attempt_count,
        });
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(claims)
}
