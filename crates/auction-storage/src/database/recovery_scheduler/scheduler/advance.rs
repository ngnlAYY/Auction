//! Recovery case 到期推进；事务入口和外部副作用由父模块统一约束。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
#[derive(Debug)]
pub(super) struct DueRecoveryRow {
    pub(super) id: Uuid,
    pub(super) stage: String,
    pub(super) state_version: i64,
    pub(super) reminder_count: i32,
    pub(super) reminder_limit: i32,
    pub(super) world_id: String,
    pub(super) payment_execution_state: Option<String>,
    pub(super) payment_completed_at: Option<DateTime<Utc>>,
    pub(super) equipment_execution_state: Option<String>,
    pub(super) equipment_completed_at: Option<DateTime<Utc>>,
    pub(super) created_at: DateTime<Utc>,
}
#[cfg(feature = "postgres")]
pub(super) async fn advance_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<RecoveryCaseAdvance>, auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;

    let Some(row) = sqlx::query(
        r#"SELECT recovery.id,
                  recovery.stage,
                  recovery.state_version,
                  recovery.reminder_count,
                  recovery.reminder_limit,
                  recovery.world_id,
                  payment_f.execution_state AS payment_execution_state,
                  payment_f.completed_at AS payment_completed_at,
                  equipment_f.execution_state AS equipment_execution_state,
                  equipment_f.completed_at AS equipment_completed_at,
                  recovery.created_at
           FROM public.settlement_recoveries AS recovery
           LEFT JOIN public.settlement_obligation_fulfillments AS payment_f
               ON payment_f.obligation_id = recovery.payment_obligation_id
           LEFT JOIN public.settlement_obligation_fulfillments AS equipment_f
               ON equipment_f.obligation_id = recovery.equipment_obligation_id
           WHERE (recovery.next_attempt_at IS NULL OR recovery.next_attempt_at <= $1)
             AND (
                 (
                     recovery.stage IN ('awaiting_payment', 'payment_overdue')
                     AND payment_f.execution_state = 'satisfied'
                 )
                 OR (
                     recovery.stage = 'awaiting_payment'
                     AND recovery.payment_deadline_at <= $1
                 )
                 OR (
                     recovery.world_id = 'isekai'
                     AND recovery.stage = 'delivering_equipment'
                     AND equipment_f.execution_state = 'satisfied'
                 )
             )
           ORDER BY COALESCE(recovery.next_attempt_at, recovery.payment_deadline_at),
                    recovery.created_at,
                    recovery.id
           LIMIT 1
           FOR UPDATE OF recovery SKIP LOCKED"#,
    )
    .bind(now)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .map(map_due_recovery_row)
    .transpose()?
    else {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(None);
    };

    let payment_satisfied = row.payment_execution_state.as_deref() == Some("satisfied");
    let equipment_satisfied = row.equipment_execution_state.as_deref() == Some("satisfied");
    let (next_stage, next_reminder_count, completed_at, create_reminder) =
        if row.world_id == "main" && payment_satisfied {
            ("completed", row.reminder_count, row.payment_completed_at, false)
        } else if row.world_id == "isekai" && equipment_satisfied {
            ("completed", row.reminder_count, row.equipment_completed_at, false)
        } else if row.world_id == "isekai" && payment_satisfied {
            ("delivering_equipment", row.reminder_count, None, false)
        } else if row.stage == "awaiting_payment" {
            if row.reminder_count >= row.reminder_limit {
                return Err(repository_error("recovery 提醒次数已超过冻结上限"));
            }
            (
                "payment_overdue",
                row.reminder_count
                    .checked_add(1)
                    .ok_or_else(|| repository_error("recovery 提醒次数溢出"))?,
                None,
                true,
            )
        } else {
            return Err(repository_error("recovery due case 状态不支持推进"));
        };

    if next_stage == "completed" {
        let Some(completed_at) = completed_at else {
            return Err(repository_error("recovery 完成事实缺少履约时间"));
        };
        if completed_at < row.created_at || completed_at > now {
            return Err(repository_error("recovery 完成事实时间越界"));
        }
    }

    let updated = sqlx::query(
        r#"UPDATE public.settlement_recoveries
           SET stage = $1,
               reminder_count = $2,
               state_version = state_version + 1,
               next_attempt_at = NULL,
               completed_at = $3,
               updated_at = $4
           WHERE id = $5
             AND stage = $6
             AND state_version = $7
           RETURNING id, stage, reminder_count, state_version"#,
    )
    .bind(next_stage)
    .bind(next_reminder_count)
    .bind(completed_at)
    .bind(now)
    .bind(row.id)
    .bind(&row.stage)
    .bind(row.state_version)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error("recovery case 状态 fence 已失效"))?;

    if create_reminder {
        let action_id =
            deterministic_uuid(CommandId::from_uuid(row.id), "recovery-action:payment-reminder", 1);
        sqlx::query(
            r#"INSERT INTO public.settlement_recovery_actions
                   (id, recovery_id, kind, ordinal, execution_state, state_version,
                    created_at, updated_at)
               VALUES ($1, $2, 'payment_reminder', 0, 'pending', 1, $3, $3)
               ON CONFLICT (recovery_id, kind) DO NOTHING"#,
        )
        .bind(action_id)
        .bind(row.id)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;

        let stored_action: Uuid = sqlx::query(
            "SELECT id FROM public.settlement_recovery_actions
             WHERE recovery_id = $1 AND kind = 'payment_reminder' FOR UPDATE",
        )
        .bind(row.id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(map_storage_error)?
        .try_get("id")
        .map_err(map_storage_error)?;
        if stored_action != action_id {
            return Err(repository_error("recovery 提醒 action 标识冲突"));
        }
    }

    let recovery_id: Uuid = updated.try_get("id").map_err(map_storage_error)?;
    let stage: String = updated.try_get("stage").map_err(map_storage_error)?;
    let reminder_count: i32 = updated.try_get("reminder_count").map_err(map_storage_error)?;
    let state_version: i64 = updated.try_get("state_version").map_err(map_storage_error)?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(Some(RecoveryCaseAdvance {
        recovery_id: SettlementId::from_uuid(recovery_id),
        completed: stage == "completed",
        stage,
        reminder_count,
        state_version,
        advanced_at: now,
    }))
}
