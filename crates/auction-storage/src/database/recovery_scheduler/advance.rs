//! Recovery case 到期推进；事务入口和外部副作用由父模块统一约束。

use {
    crate::database::recovery_scheduler::{
        map_due_recovery_row, map_storage_error, repository_error,
    },
    auction_application::recovery::RecoveryCaseAdvance,
    auction_domain::{CommandId, SettlementId, deterministic_uuid},
    chrono::{DateTime, Utc},
    sqlx::{Postgres, Row},
    uuid::Uuid,
};

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
    /// 付款投递已有自动或人工发送证明；没有证明时逾期提醒只延后。
    pub(super) payment_sent_proven: bool,
}
/// 付款邮件缺乏发送证明时的延后间隔；只影响重试节奏，不改冻结期限。
const PAYMENT_MAIL_UNPROVEN_RETRY_MINUTES: i64 = 15;

pub(super) async fn advance_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<RecoveryCaseAdvance>, auction_application::core::RepositoryError> {
    pool.observe_transaction(auction_observability::DatabaseTransactionOwner::Recovery, async {
        let mut transaction = pool
            .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
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
                  COALESCE(public.delivery_has_frozen_sent_proof(
                      payment_d.id, payment_d.result_metadata, payment_d.external_reference
                  ), false) AS payment_sent_proven,
                  equipment_f.execution_state AS equipment_execution_state,
                  equipment_f.completed_at AS equipment_completed_at,
                  recovery.created_at
           FROM public.settlement_recoveries AS recovery
           LEFT JOIN public.settlement_obligation_fulfillments AS payment_f
               ON payment_f.obligation_id = recovery.payment_obligation_id
           LEFT JOIN public.deliveries AS payment_d
               ON payment_d.id = payment_f.delivery_id
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
        // 阶段推进只在领域层判定；这里只负责把锁定行投影成事实并执行结果分支。
        let facts = auction_domain::RecoveryAdvanceFacts {
            world: match row.world_id.as_str() {
                "main" => auction_domain::SettlementWorld::Main,
                "isekai" => auction_domain::SettlementWorld::Isekai,
                _ => return Err(repository_error("recovery world 不受支持")),
            },
            stage: auction_domain::RecoveryStage::parse(&row.stage)
                .ok_or_else(|| repository_error("recovery due case 状态不支持推进"))?,
            payment_satisfied,
            payment_completed_at: row.payment_completed_at,
            equipment_satisfied,
            equipment_completed_at: row.equipment_completed_at,
            reminder_count: row.reminder_count,
            reminder_limit: row.reminder_limit,
            payment_sent_proven: row.payment_sent_proven,
            created_at: row.created_at,
            observed_at: now,
        };
        let (next_stage, next_reminder_count, completed_at, create_reminder) =
            match auction_domain::next_recovery_advance(&facts)
                .map_err(|error| repository_error(&error.to_string()))?
            {
                auction_domain::RecoveryAdvance::DeferPaymentReminder => {
                    // 付款邮件（含 COD/载体）尚无发送证明时不提前提醒，
                    // 只延后本轮并记录可观测审计；不修改冻结的 payment_deadline_at。
                    sqlx::query(
                        r#"UPDATE public.settlement_recoveries
                           SET next_attempt_at = $2, updated_at = $3
                           WHERE id = $1 AND stage = $4 AND state_version = $5"#,
                    )
                    .bind(row.id)
                    .bind(now + chrono::Duration::minutes(PAYMENT_MAIL_UNPROVEN_RETRY_MINUTES))
                    .bind(now)
                    .bind(&row.stage)
                    .bind(row.state_version)
                    .execute(&mut *transaction)
                    .await
                    .map_err(map_storage_error)?;
                    sqlx::query(
                        r#"INSERT INTO public.audit_records
                               (id, occurred_at, actor_type, actor_id, action, entity_type,
                                entity_id, result, source, metadata)
                           VALUES ($1, $2, 'system', 'auction-recovery',
                                   'recovery.payment_reminder_deferred',
                                   'settlement_recovery', $3, 'success', 'worker', $4)"#,
                    )
                    .bind(Uuid::new_v4())
                    .bind(now)
                    .bind(row.id.to_string())
                    .bind(serde_json::json!({
                        "reason": "payment_mail_not_sent",
                        "stage": row.stage,
                        "deadline_passed": true,
                    }))
                    .execute(&mut *transaction)
                    .await
                    .map_err(map_storage_error)?;
                    transaction.commit().await.map_err(map_storage_error)?;
                    return Ok(Some(RecoveryCaseAdvance {
                        recovery_id: SettlementId::from_uuid(row.id),
                        stage: row.stage,
                        reminder_count: row.reminder_count,
                        state_version: row.state_version,
                        completed: false,
                        advanced_at: now,
                    }));
                }
                auction_domain::RecoveryAdvance::Completed { completed_at } => {
                    ("completed", row.reminder_count, Some(completed_at), false)
                }
                auction_domain::RecoveryAdvance::DeliverEquipment => {
                    ("delivering_equipment", row.reminder_count, None, false)
                }
                auction_domain::RecoveryAdvance::RemindPayment { reminder_count } => {
                    ("payment_overdue", reminder_count, None, true)
                }
            };

        if next_stage == "completed" && completed_at.is_none() {
            return Err(repository_error("recovery 完成事实缺少履约时间"));
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
            let action_id = deterministic_uuid(
                CommandId::from_uuid(row.id),
                "recovery-action:payment-reminder",
                1,
            );
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
    })
    .await
}
