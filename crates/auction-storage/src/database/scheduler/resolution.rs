//! Worker 调度完成/失败的租约 fence 收口。

use auction_application::scheduling::{ScheduledRoundClaim, ScheduledRoundPhase};
use chrono::{DateTime, Utc};

use sqlx::Postgres;

use super::mapping::{map_storage_error, repository_error_owned};
use crate::database::TrackedPool;

pub(super) async fn complete_postgres(
    pool: &TrackedPool<Postgres>,
    claim: &ScheduledRoundClaim,
    completed_at: DateTime<Utc>,
) -> Result<(), auction_application::core::RepositoryError> {
    let (sql, phase) = match claim.phase {
        ScheduledRoundPhase::Preview => (
            "UPDATE public.scheduled_rounds
             SET preview_command_id = $3, preview_completed_at = $4,
                 state_version = state_version + 1,
                 lease_owner = NULL, lease_until = NULL, updated_at = CURRENT_TIMESTAMP
             WHERE id = $1 AND status = 'active' AND lease_owner = $2
               AND lease_until = $5 AND state_version = $6 AND auction_id = $7
               AND preview_completed_at IS NULL AND final_completed_at IS NULL",
            "preview",
        ),
        ScheduledRoundPhase::Open => (
            "UPDATE public.scheduled_rounds
             SET open_command_id = $3, open_completed_at = $4,
                 state_version = state_version + 1,
                 lease_owner = NULL, lease_until = NULL, updated_at = CURRENT_TIMESTAMP
             WHERE id = $1 AND status = 'active' AND lease_owner = $2
                   AND lease_until = $5 AND state_version = $6 AND auction_id = $7
                   AND preview_completed_at IS NOT NULL AND open_completed_at IS NULL
                   AND final_completed_at IS NULL",
            "open",
        ),
        ScheduledRoundPhase::Final => (
            "UPDATE public.scheduled_rounds
             SET final_command_id = $3, final_completed_at = $4,
                 state_version = state_version + 1,
                 lease_owner = NULL, lease_until = NULL, updated_at = CURRENT_TIMESTAMP
             WHERE id = $1 AND status = 'active' AND lease_owner = $2
                   AND lease_until = $5 AND state_version = $6 AND auction_id = $7
                   AND preview_completed_at IS NOT NULL AND open_completed_at IS NOT NULL
                   AND final_completed_at IS NULL",
            "final",
        ),
    };
    let result = sqlx::query(sql)
        .bind(claim.round_id)
        .bind(&claim.worker_id)
        .bind(claim.command_id.as_uuid())
        .bind(completed_at)
        .bind(claim.lease_until)
        .bind(claim.state_version)
        .bind(claim.auction_id.as_uuid())
        .execute(pool)
        .await
        .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        // 生命周期事务已原子完成任一阶段；只能确认同一轮次、拍卖和命令的持久化证明。
        let completed = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM public.scheduled_rounds
             WHERE id = $1 AND auction_id = $2
               AND CASE $4
                 WHEN 'preview' THEN preview_command_id = $3 AND preview_completed_at IS NOT NULL
                 WHEN 'open' THEN open_command_id = $3 AND open_completed_at IS NOT NULL
                 WHEN 'final' THEN final_command_id = $3 AND final_completed_at IS NOT NULL
                 ELSE false
               END)",
        )
        .bind(claim.round_id)
        .bind(claim.auction_id.as_uuid())
        .bind(claim.command_id.as_uuid())
        .bind(phase)
        .fetch_one(pool)
        .await
        .map_err(map_storage_error)?;
        if !completed {
            return Err(repository_error_owned(&format!("Worker {phase} 阶段完成租约已失效")));
        }
    }
    Ok(())
}

pub(super) async fn fail_postgres(
    pool: &TrackedPool<Postgres>,
    claim: &ScheduledRoundClaim,
    retry_at: DateTime<Utc>,
    failure_code: &str,
    safe_detail: &str,
) -> Result<(), auction_application::core::RepositoryError> {
    let result = match claim.phase {
        ScheduledRoundPhase::Preview => {
            sqlx::query(
                "UPDATE public.scheduled_rounds
                 SET lease_owner = NULL, lease_until = $3, last_failure_code = $4,
                     state_version = state_version + 1,
                     last_failure_detail = $5, updated_at = CURRENT_TIMESTAMP
                 WHERE id = $1 AND status = 'active' AND lease_owner = $2
                   AND lease_until = $6 AND state_version = $7
                   AND preview_completed_at IS NULL AND final_completed_at IS NULL",
            )
            .bind(claim.round_id)
            .bind(&claim.worker_id)
            .bind(retry_at)
            .bind(failure_code)
            .bind(safe_detail)
            .bind(claim.lease_until)
            .bind(claim.state_version)
            .execute(pool)
            .await
        }
        ScheduledRoundPhase::Open => {
            sqlx::query(
                "UPDATE public.scheduled_rounds
                 SET lease_owner = NULL, lease_until = $3, last_failure_code = $4,
                     state_version = state_version + 1,
                     last_failure_detail = $5, updated_at = CURRENT_TIMESTAMP
                 WHERE id = $1 AND status = 'active' AND lease_owner = $2
                   AND lease_until = $6 AND state_version = $7
                   AND preview_completed_at IS NOT NULL AND open_completed_at IS NULL
                   AND final_completed_at IS NULL",
            )
            .bind(claim.round_id)
            .bind(&claim.worker_id)
            .bind(retry_at)
            .bind(failure_code)
            .bind(safe_detail)
            .bind(claim.lease_until)
            .bind(claim.state_version)
            .execute(pool)
            .await
        }
        ScheduledRoundPhase::Final => {
            sqlx::query(
                "UPDATE public.scheduled_rounds
                 SET lease_owner = NULL, lease_until = $3, last_failure_code = $4,
                     state_version = state_version + 1,
                     last_failure_detail = $5, updated_at = CURRENT_TIMESTAMP
                 WHERE id = $1 AND status = 'active' AND lease_owner = $2
                   AND lease_until = $6 AND state_version = $7
                   AND preview_completed_at IS NOT NULL AND open_completed_at IS NOT NULL
                   AND final_completed_at IS NULL",
            )
            .bind(claim.round_id)
            .bind(&claim.worker_id)
            .bind(retry_at)
            .bind(failure_code)
            .bind(safe_detail)
            .bind(claim.lease_until)
            .bind(claim.state_version)
            .execute(pool)
            .await
        }
    }
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned("Worker 失败记录租约已失效"));
    }
    Ok(())
}
