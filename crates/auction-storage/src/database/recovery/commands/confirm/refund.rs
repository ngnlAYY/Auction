//! Worker 对 `equipment_recovered` 之后买方退款的两种完成收口，由恢复 owner 各自追加独立的完成事实：
//!
//! - 退款履约已满足：审计 `auction.recovery.refund_completed`；
//! - 退款邮件被拒收退回、经退件案件精确领回并代管结案（履约 `failed/settlement_return_held`，案件
//!   `held` 且有领回证明）：审计 `auction.recovery.refund_custody_completed`，不冒充退款已完成。
//!   退款负债与资金用途保持不变（代管负债由对账单列，原退款分配仍是 refund 用途），之后从代管
//!   补寄照常经退件案件执行，不受恢复记录已完成影响。

use auction_application::recovery::{RecoveryCommandError, RecoveryResolutionAdvance};
use auction_time::Timestamp;
use serde_json::json;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::database::recovery::commands::confirm::insert_worker_recovery_audit;
use crate::database::recovery::commands::mapping::is_conflict;
use crate::database::recovery::commands::mapping::map_sql_error;
use crate::database::recovery::commands::scope::lock_recovery;

pub(in crate::database::recovery::commands) async fn complete_refund_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    now: Timestamp,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError> {
    let satisfied = crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::Recovery,
        crate::database::SERIALIZABLE_ATTEMPTS,
        is_conflict,
        || complete_refund_postgres_once(pool, now),
    )
    .await?;
    if satisfied.is_some() {
        return Ok(satisfied);
    }
    crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::Recovery,
        crate::database::SERIALIZABLE_ATTEMPTS,
        is_conflict,
        || complete_refund_custody_once(pool, now),
    )
    .await
}

pub(in crate::database::recovery::commands) async fn complete_refund_postgres_once(
    pool: &crate::database::TrackedPool<Postgres>,
    now: Timestamp,
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
    let candidate_completed_at: Option<Timestamp> =
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
    let stored_completed_at: Option<Timestamp> =
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
    let updated_completed_at: Option<Timestamp> =
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
    }))
}

/// 退款被退回并代管结案后的恢复完成：退款履约以 `settlement_return_held` 终结、退回的正是该履约
/// 当前派发，绑定它的退件案件已代管结案且领回证明已持久化。恢复记录推进为 `completed`（完成时刻
/// 取领回证明时刻），同事务追加独立的完成事实；不改写退款履约、资金分配或退件案件，原权利人的
/// 退款权益由代管负债承接（不变量 8、10）。锁序：恢复记录 → 履约（与满足路径相同）。
async fn complete_refund_custody_once(
    pool: &crate::database::TrackedPool<Postgres>,
    now: Timestamp,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError> {
    let mut transaction =
        pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await.map_err(map_sql_error)?;
    let Some(candidate) = sqlx::query(
        r#"SELECT recovery.id, c.return_mail_id, take.take_delivery_id,
                  take.original_delivery_id, take.proven_at
           FROM public.settlement_recoveries AS recovery
           JOIN public.settlement_obligation_fulfillments AS fulfillment
               ON fulfillment.obligation_id = recovery.buyer_refund_obligation_id
           JOIN public.settlement_return_cases AS c
               ON c.obligation_id = recovery.buyer_refund_obligation_id
              AND c.delivery_id = fulfillment.delivery_id
           JOIN public.settlement_return_takes AS take
               ON take.return_mail_id = c.return_mail_id
              AND take.original_delivery_id = fulfillment.delivery_id
           WHERE recovery.stage = 'refunding'
             AND recovery.resolution = 'equipment_recovered'
             AND recovery.blocking_code IS NULL
             AND recovery.manual_reconcile_delivery_id IS NULL
             AND recovery.relisted_auction_id IS NOT NULL
             AND recovery.relisted_lot_id IS NOT NULL
             AND fulfillment.execution_state = 'failed'
             AND fulfillment.last_error_code = 'settlement_return_held'
             AND c.status = 'held'
             AND take.proven_at <= $1
           ORDER BY take.proven_at, recovery.created_at, recovery.id
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
    let return_mail_id: Uuid = candidate.try_get("return_mail_id").map_err(map_sql_error)?;
    let take_delivery_id: Uuid = candidate.try_get("take_delivery_id").map_err(map_sql_error)?;
    let returned_delivery: Uuid =
        candidate.try_get("original_delivery_id").map_err(map_sql_error)?;
    let proven_at: Timestamp = candidate.try_get("proven_at").map_err(map_sql_error)?;
    let recovery = lock_recovery(&mut transaction, recovery_id).await?;
    if recovery.stage != "refunding"
        || recovery.resolution.as_deref() != Some("equipment_recovered")
        || recovery.relisted_auction_id.is_none()
        || recovery.relisted_lot_id.is_none()
        || recovery.blocking_code.is_some()
        || recovery.manual_reconcile_delivery_id.is_some()
        || proven_at < recovery.created_at
        || proven_at > now
    {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    // 锁内复核：退款履约仍以代管结案终结且指向被领回的那次派发；被并发补寄改写时整体回滚。
    let held: Option<Uuid> = sqlx::query_scalar(
        r#"SELECT obligation_id FROM public.settlement_obligation_fulfillments
           WHERE obligation_id = $1 AND execution_state = 'failed'
             AND last_error_code = 'settlement_return_held' AND delivery_id = $2
           FOR UPDATE"#,
    )
    .bind(recovery.buyer_refund_obligation_id)
    .bind(returned_delivery)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_sql_error)?;
    if held.is_none() {
        return Err(RecoveryCommandError::StaleState);
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
           RETURNING stage, resolution, state_version"#,
    )
    .bind(proven_at)
    .bind(now)
    .bind(recovery.id)
    .bind(recovery.state_version)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::StaleState)?;
    let stage: String = updated.try_get("stage").map_err(map_sql_error)?;
    let resolution: Option<String> = updated.try_get("resolution").map_err(map_sql_error)?;
    let state_version: i64 = updated.try_get("state_version").map_err(map_sql_error)?;
    insert_worker_recovery_audit(
        &mut transaction,
        recovery.id,
        "auction.recovery.refund_custody_completed",
        now,
        json!({
            "buyer_refund_obligation_id": recovery.buyer_refund_obligation_id,
            "return_mail_id": return_mail_id,
            "take_delivery_id": take_delivery_id,
            "returned_delivery_id": returned_delivery,
            "take_proven_at": proven_at,
            "previous_stage": recovery.stage,
            "next_stage": stage,
            "previous_state_version": recovery.state_version,
            "state_version": state_version,
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
    }))
}
