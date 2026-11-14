//! Worker buyer refund fulfillment 的完成收口。

use auction_application::recovery::{RecoveryCommandError, RecoveryResolutionAdvance};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::database::recovery::commands::confirm::insert_worker_recovery_audit;
use crate::database::recovery::commands::mapping::is_conflict;
use crate::database::recovery::commands::mapping::map_sql_error;
use crate::database::recovery::commands::scope::lock_recovery;

pub(in crate::database::recovery::commands) async fn complete_refund_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError> {
    crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::Recovery,
        crate::database::SERIALIZABLE_ATTEMPTS,
        is_conflict,
        || complete_refund_postgres_once(pool, now),
    )
    .await
}

pub(in crate::database::recovery::commands) async fn complete_refund_postgres_once(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError> {
    let mut transaction =
        pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await.map_err(map_sql_error)?;

    let Some(candidate) = sqlx::query(
        r#"SELECT recovery.id,
                  fulfillment.completed_at AS refund_completed_at
           FROM public.settlement_recoveries AS recovery
           INNER JOIN public.settlement_obligation_fulfillments AS fulfillment
               ON fulfillment.obligation_id = recovery.buyer_refund_obligation_id
           WHERE recovery.stage = 'refunding'
             AND recovery.resolution = 'equipment_recovered'
             AND recovery.blocking_code IS NULL
             AND recovery.manual_reconcile_delivery_id IS NULL
             AND recovery.relisted_auction_id IS NOT NULL
             AND recovery.relisted_lot_id IS NOT NULL
             AND fulfillment.execution_state = 'satisfied'
             AND fulfillment.completed_at IS NOT NULL
             AND fulfillment.completed_at <= $1
           ORDER BY fulfillment.completed_at, recovery.created_at, recovery.id
           LIMIT 1
           FOR UPDATE OF recovery SKIP LOCKED"#,
    )
    .bind(now)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_sql_error)?
    else {
        transaction.commit().await.map_err(map_sql_error)?;
        return Ok(None);
    };

    let recovery_id: Uuid = candidate.try_get("id").map_err(map_sql_error)?;
    let candidate_completed_at: Option<DateTime<Utc>> =
        candidate.try_get("refund_completed_at").map_err(map_sql_error)?;
    let completed_at = candidate_completed_at.ok_or(RecoveryCommandError::InvalidStoredResult)?;
    let recovery = lock_recovery(&mut transaction, recovery_id).await?;
    if recovery.stage != "refunding"
        || recovery.resolution.as_deref() != Some("equipment_recovered")
        || recovery.relisted_auction_id.is_none()
        || recovery.relisted_lot_id.is_none()
        || recovery.blocking_code.is_some()
        || recovery.manual_reconcile_delivery_id.is_some()
        || completed_at < recovery.created_at
        || completed_at > now
    {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }

    let fulfillment = sqlx::query(
        r#"SELECT execution_state, completed_at
           FROM public.settlement_obligation_fulfillments
           WHERE obligation_id = $1
           FOR UPDATE"#,
    )
    .bind(recovery.buyer_refund_obligation_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::InvalidStoredResult)?;
    let execution_state: String = fulfillment.try_get("execution_state").map_err(map_sql_error)?;
    let stored_completed_at: Option<DateTime<Utc>> =
        fulfillment.try_get("completed_at").map_err(map_sql_error)?;
    if execution_state != "satisfied" || stored_completed_at != Some(completed_at) {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }

    let updated = sqlx::query(
        r#"UPDATE public.settlement_recoveries
           SET stage = 'completed',
               next_attempt_at = NULL,
               completed_at = $1,
               state_version = state_version + 1,
               updated_at = $2
           WHERE id = $3
             AND stage = 'refunding'
             AND resolution = 'equipment_recovered'
             AND state_version = $4
             AND relisted_auction_id IS NOT NULL
             AND relisted_lot_id IS NOT NULL
             AND blocking_code IS NULL
             AND manual_reconcile_delivery_id IS NULL
             AND EXISTS (
                 SELECT 1
                 FROM public.settlement_obligation_fulfillments AS fulfillment
                 WHERE fulfillment.obligation_id = $5
                   AND fulfillment.execution_state = 'satisfied'
                   AND fulfillment.completed_at = $1
             )
           RETURNING stage, resolution, state_version, completed_at"#,
    )
    .bind(completed_at)
    .bind(now)
    .bind(recovery.id)
    .bind(recovery.state_version)
    .bind(recovery.buyer_refund_obligation_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::StaleState)?;
    let stage: String = updated.try_get("stage").map_err(map_sql_error)?;
    let resolution: Option<String> = updated.try_get("resolution").map_err(map_sql_error)?;
    let state_version: i64 = updated.try_get("state_version").map_err(map_sql_error)?;
    let updated_completed_at: Option<DateTime<Utc>> =
        updated.try_get("completed_at").map_err(map_sql_error)?;
    if stage != "completed"
        || resolution.as_deref() != Some("equipment_recovered")
        || updated_completed_at != Some(completed_at)
    {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }

    insert_worker_recovery_audit(
        &mut transaction,
        recovery.id,
        "auction.recovery.refund_completed",
        now,
        json!({
            "buyer_refund_obligation_id": recovery.buyer_refund_obligation_id,
            "previous_stage": recovery.stage,
            "next_stage": stage,
            "previous_state_version": recovery.state_version,
            "state_version": state_version,
            "refund_completed_at": completed_at,
        }),
    )
    .await?;
    transaction.commit().await.map_err(map_sql_error)?;
    Ok(Some(RecoveryResolutionAdvance {
        recovery_id: recovery.id.into(),
        stage,
        resolution,
        relisted_auction_id: recovery.relisted_auction_id,
        relisted_lot_id: recovery.relisted_lot_id,
        state_version,
        completed: true,
        advanced_at: now,
    }))
}
