//! PostgreSQL recovery 确认事务适配器。
//!
//! A14 只允许在锁定 recovery、原拍卖、lot/equipment、相关 obligation 和 action 后执行。命令
//! 账本、退款激活、guarded relist、状态版本、审计以及预览目录事件都在同一 Serializable
//! 事务内完成；SQLite Lite 明确返回能力缺失。

#[cfg(feature = "postgres")]
use std::collections::BTreeMap;

use auction_application::{
    ApplicationError, ApplicationFuture, RecoveryCommandRepository, RecoveryConfirmationCommand,
    RecoveryConfirmationResult, RecoveryResolutionAdvance, RecoveryResolutionStore,
};
#[cfg(feature = "postgres")]
use auction_application::{RecoveryCommandError, RecoveryConfirmationAction};
#[cfg(feature = "postgres")]
use auction_domain::{CommandId, deterministic_uuid};
use chrono::Duration as ChronoDuration;
#[cfg(feature = "postgres")]
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use serde_json::{Value, json};
#[cfg(feature = "postgres")]
use sqlx::{Postgres, Row, Transaction};
#[cfg(feature = "postgres")]
use uuid::Uuid;

#[cfg(feature = "postgres")]
use crate::database::audit_health::AuditWriteResult as _;

use super::DatabaseStore;

impl RecoveryResolutionStore for DatabaseStore {
    fn advance_due_relist(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        retry_after: ChronoDuration,
    ) -> ApplicationFuture<'_, Option<RecoveryResolutionAdvance>> {
        Box::pin(async move {
            if now.timestamp_nanos_opt().is_none()
                || retry_after <= ChronoDuration::zero()
                || retry_after > ChronoDuration::hours(24)
            {
                return Err(ApplicationError::Repository(
                    auction_application::RepositoryError::Failed(
                        "recovery relist 调度参数无效".to_owned(),
                    ),
                ));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => advance_relist_postgres(store.pool(), now, retry_after)
                    .await
                    .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery relist Worker 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn complete_due_refund(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ApplicationFuture<'_, Option<RecoveryResolutionAdvance>> {
        Box::pin(async move {
            if now.timestamp_nanos_opt().is_none() {
                return Err(ApplicationError::Repository(
                    auction_application::RepositoryError::Failed(
                        "recovery refund 调度时间无效".to_owned(),
                    ),
                ));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => complete_refund_postgres(store.pool(), now)
                    .await
                    .map_err(ApplicationError::from),
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery refund Worker 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

impl RecoveryCommandRepository for DatabaseStore {
    fn confirm_recovery(
        &self,
        command: RecoveryConfirmationCommand,
    ) -> ApplicationFuture<'_, RecoveryConfirmationResult> {
        Box::pin(async move {
            if let Err(error) = command.validate() {
                return Err(error.into());
            }

            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    confirm_postgres(store.pool(), command).await.map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery 确认仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn confirm_postgres(
    pool: &super::TrackedPool<Postgres>,
    command: RecoveryConfirmationCommand,
) -> Result<RecoveryConfirmationResult, RecoveryCommandError> {
    for attempt in 0..3_u8 {
        match confirm_postgres_once(pool, command.clone()).await {
            Err(RecoveryCommandError::SerializationConflict) if attempt < 2 => continue,
            result => return result,
        }
    }
    Err(RecoveryCommandError::SerializationConflict)
}

#[cfg(feature = "postgres")]
async fn confirm_postgres_once(
    pool: &super::TrackedPool<Postgres>,
    command: RecoveryConfirmationCommand,
) -> Result<RecoveryConfirmationResult, RecoveryCommandError> {
    let mut transaction = pool.begin().await.map_err(map_sql_error)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(map_sql_error)?;

    if let Some(result) = load_replayed_command(&mut transaction, &command).await? {
        transaction.commit().await.map_err(map_sql_error)?;
        return Ok(result);
    }

    let scope = load_recovery_scope(&mut transaction, command.recovery_id.as_uuid()).await?;
    lock_world(&mut transaction, &scope.world_id).await?;
    let auction = lock_auction(&mut transaction, scope.auction_id).await?;
    if auction.world_id != scope.world_id || auction.status != "settled" {
        return Err(RecoveryCommandError::NotConfirmable);
    }

    let recovery = lock_recovery(&mut transaction, command.recovery_id.as_uuid()).await?;
    validate_recovery_row(&recovery, &scope, &command)?;

    let lot = lock_original_lot(
        &mut transaction,
        recovery.auction_id,
        recovery.lot_id,
        recovery.equipment_id,
    )
    .await?;
    if lot.status != "sold"
        || lot.submission_id.is_none()
        || lot.start_price <= 0
        || lot.equipment_world_id != recovery.world_id
    {
        return Err(RecoveryCommandError::NotConfirmable);
    }

    let obligations = lock_obligations(&mut transaction, &recovery).await?;
    let actions = lock_actions(&mut transaction, recovery.id).await?;
    let selected_action = validate_actions(&recovery, &actions, command.action)?;
    validate_custody_evidence(&recovery, selected_action)?;
    validate_obligations(&mut transaction, &recovery, &obligations, command.action).await?;

    let now = database_now(&mut transaction).await?;
    if command.action == RecoveryConfirmationAction::EquipmentRecovered {
        activate_buyer_refund(&mut transaction, &recovery, now).await?;
    }

    let relist =
        relist_recovery(&mut transaction, &recovery, &lot, command.command_id, command.action, now)
            .await?;

    let next_state_version = recovery
        .state_version
        .checked_add(1)
        .ok_or_else(|| RecoveryCommandError::Failed("recovery 状态版本溢出".to_owned()))?;
    let updated_state_version =
        update_recovery(&mut transaction, &recovery, &relist, next_state_version, now).await?;
    if updated_state_version != next_state_version {
        return Err(RecoveryCommandError::StaleState);
    }

    let result = RecoveryConfirmationResult {
        recovery_id: recovery.id.into(),
        action: command.action,
        stage: relist.stage.to_owned(),
        resolution: command.action.as_str().to_owned(),
        state_version: updated_state_version,
        relisted_auction_id: relist.auction_id.map(auction_domain::AuctionId::from_uuid),
        relisted_lot_id: relist.lot_id.map(auction_domain::LotId::from_uuid),
    };
    result.validate()?;

    insert_recovery_command(&mut transaction, &command, &result, now).await?;
    insert_recovery_audit(&mut transaction, &command, &recovery, selected_action, &result).await?;

    transaction.commit().await.map_err(map_sql_error)?;
    Ok(result)
}

#[cfg(feature = "postgres")]
async fn advance_relist_postgres(
    pool: &super::TrackedPool<Postgres>,
    now: DateTime<Utc>,
    retry_after: ChronoDuration,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError> {
    for attempt in 0..3_u8 {
        match advance_relist_postgres_once(pool, now, retry_after).await {
            Err(RecoveryCommandError::SerializationConflict) if attempt < 2 => continue,
            result => return result,
        }
    }
    Err(RecoveryCommandError::SerializationConflict)
}

#[cfg(feature = "postgres")]
async fn advance_relist_postgres_once(
    pool: &super::TrackedPool<Postgres>,
    now: DateTime<Utc>,
    retry_after: ChronoDuration,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError> {
    let mut transaction = pool.begin().await.map_err(map_sql_error)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(map_sql_error)?;

    // 先锁 world，再锁 recovery，保持与 Admin recovery 确认的锁顺序，避免 relist 与人工确认
    // 在同一 world 上形成 recovery/world 反向等待。
    let Some(world_row) = sqlx::query(
        r#"SELECT world_id
           FROM public.settlement_recoveries
           WHERE stage = 'relist_pending'
             AND resolution IS NOT NULL
             AND blocking_code IS NULL
             AND manual_reconcile_delivery_id IS NULL
             AND custody_proof_kind IS NOT NULL
             AND relisted_auction_id IS NULL
             AND relisted_lot_id IS NULL
             AND (next_attempt_at IS NULL OR next_attempt_at <= $1)
           ORDER BY COALESCE(next_attempt_at, created_at), created_at, id
           LIMIT 1"#,
    )
    .bind(now)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_sql_error)?
    else {
        transaction.commit().await.map_err(map_sql_error)?;
        return Ok(None);
    };

    let world_id: String = world_row.try_get("world_id").map_err(map_sql_error)?;
    lock_world(&mut transaction, &world_id).await?;
    let Some(candidate) = sqlx::query(
        r#"SELECT id, auction_id
           FROM public.settlement_recoveries
           WHERE world_id = $1
             AND stage = 'relist_pending'
             AND resolution IS NOT NULL
             AND blocking_code IS NULL
             AND manual_reconcile_delivery_id IS NULL
             AND custody_proof_kind IS NOT NULL
             AND relisted_auction_id IS NULL
             AND relisted_lot_id IS NULL
           AND (next_attempt_at IS NULL OR next_attempt_at <= $2)
           ORDER BY COALESCE(next_attempt_at, created_at), created_at, id
           LIMIT 1"#,
    )
    .bind(&world_id)
    .bind(now)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_sql_error)?
    else {
        transaction.commit().await.map_err(map_sql_error)?;
        return Ok(None);
    };

    let recovery_id: Uuid = candidate.try_get("id").map_err(map_sql_error)?;
    let candidate_auction_id: Uuid = candidate.try_get("auction_id").map_err(map_sql_error)?;
    let auction = lock_auction(&mut transaction, candidate_auction_id).await?;
    let recovery = lock_recovery(&mut transaction, recovery_id).await?;
    let Some(resolution) = recovery.resolution.as_deref() else {
        return Err(RecoveryCommandError::InvalidStoredResult);
    };
    let action = RecoveryConfirmationAction::parse(resolution)
        .ok_or(RecoveryCommandError::InvalidStoredResult)?;
    if recovery.stage != "relist_pending"
        || recovery.blocking_code.is_some()
        || recovery.manual_reconcile_delivery_id.is_some()
        || recovery.custody_proof_kind.is_none()
        || recovery.relisted_auction_id.is_some()
        || recovery.relisted_lot_id.is_some()
    {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }

    if recovery.world_id != world_id {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    if candidate_auction_id != recovery.auction_id
        || auction.world_id != recovery.world_id
        || auction.status != "settled"
    {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    let lot = lock_original_lot(
        &mut transaction,
        recovery.auction_id,
        recovery.lot_id,
        recovery.equipment_id,
    )
    .await?;
    if lot.status != "sold"
        || lot.submission_id.is_none()
        || lot.start_price <= 0
        || lot.equipment_world_id != recovery.world_id
    {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }

    // 自动重挂仍必须复用人工确认使用的 action/custody 证明，不能只相信 recovery resolution 字段。
    let actions = lock_actions(&mut transaction, recovery.id).await?;
    let selected_action = validate_actions(&recovery, &actions, action)?;
    validate_custody_evidence(&recovery, selected_action)?;

    let command_id = scheduled_relist_command_id(recovery.id);
    let relist =
        relist_recovery(&mut transaction, &recovery, &lot, command_id, action, now).await?;
    if relist.auction_id.is_none() && relist.lot_id.is_none() {
        let retry_at = now.checked_add_signed(retry_after).ok_or_else(|| {
            RecoveryCommandError::Failed("recovery relist 重试时间溢出".to_owned())
        })?;
        let next_state_version =
            defer_recovery_relist(&mut transaction, &recovery, relist.resolution, retry_at, now)
                .await?;
        transaction.commit().await.map_err(map_sql_error)?;
        return Ok(Some(RecoveryResolutionAdvance {
            recovery_id: recovery.id.into(),
            stage: "relist_pending".to_owned(),
            resolution: Some(relist.resolution.to_owned()),
            relisted_auction_id: None,
            relisted_lot_id: None,
            state_version: next_state_version,
            completed: false,
            advanced_at: now,
        }));
    }

    let (Some(relisted_auction_id), Some(relisted_lot_id)) = (relist.auction_id, relist.lot_id)
    else {
        return Err(RecoveryCommandError::InvalidStoredResult);
    };
    let next_state_version = recovery
        .state_version
        .checked_add(1)
        .ok_or_else(|| RecoveryCommandError::Failed("recovery 状态版本溢出".to_owned()))?;
    let updated_state_version = complete_recovery_relist(
        &mut transaction,
        &recovery,
        &relist,
        relisted_auction_id,
        relisted_lot_id,
        next_state_version,
        now,
    )
    .await?;
    if updated_state_version != next_state_version {
        return Err(RecoveryCommandError::StaleState);
    }
    insert_worker_recovery_audit(
        &mut transaction,
        recovery.id,
        "auction.recovery.relisted",
        now,
        json!({
            "origin_command_id": command_id.as_uuid(),
            "previous_stage": recovery.stage,
            "next_stage": relist.stage,
            "previous_state_version": recovery.state_version,
            "state_version": updated_state_version,
            "resolution": relist.resolution,
            "relisted_auction_id": relisted_auction_id,
            "relisted_lot_id": relisted_lot_id,
        }),
    )
    .await?;
    transaction.commit().await.map_err(map_sql_error)?;
    Ok(Some(RecoveryResolutionAdvance {
        recovery_id: recovery.id.into(),
        stage: relist.stage.to_owned(),
        resolution: Some(relist.resolution.to_owned()),
        relisted_auction_id: Some(relisted_auction_id),
        relisted_lot_id: Some(relisted_lot_id),
        state_version: updated_state_version,
        completed: relist.stage == "completed",
        advanced_at: now,
    }))
}

#[cfg(feature = "postgres")]
async fn complete_refund_postgres(
    pool: &super::TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError> {
    for attempt in 0..3_u8 {
        match complete_refund_postgres_once(pool, now).await {
            Err(RecoveryCommandError::SerializationConflict) if attempt < 2 => continue,
            result => return result,
        }
    }
    Err(RecoveryCommandError::SerializationConflict)
}

#[cfg(feature = "postgres")]
async fn complete_refund_postgres_once(
    pool: &super::TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError> {
    let mut transaction = pool.begin().await.map_err(map_sql_error)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(map_sql_error)?;

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

#[cfg(feature = "postgres")]
fn scheduled_relist_command_id(recovery_id: Uuid) -> CommandId {
    CommandId::from_uuid(deterministic_uuid(
        CommandId::from_uuid(recovery_id),
        "recovery-relist-command",
        1,
    ))
}

#[cfg(feature = "postgres")]
async fn defer_recovery_relist(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    resolution: &str,
    retry_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<i64, RecoveryCommandError> {
    let state_version = sqlx::query(
        r#"UPDATE public.settlement_recoveries
           SET next_attempt_at = $1,
               state_version = state_version + 1,
               updated_at = $2
           WHERE id = $3
             AND stage = 'relist_pending'
             AND state_version = $4
             AND resolution = $5
             AND custody_proof_kind IS NOT NULL
             AND relisted_auction_id IS NULL
             AND relisted_lot_id IS NULL
             AND blocking_code IS NULL
             AND manual_reconcile_delivery_id IS NULL
           RETURNING state_version"#,
    )
    .bind(retry_at)
    .bind(now)
    .bind(recovery.id)
    .bind(recovery.state_version)
    .bind(resolution)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::StaleState)?;
    state_version.try_get("state_version").map_err(map_sql_error)
}

#[cfg(feature = "postgres")]
async fn complete_recovery_relist(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    relist: &RelistResult,
    relisted_auction_id: Uuid,
    relisted_lot_id: Uuid,
    next_state_version: i64,
    now: DateTime<Utc>,
) -> Result<i64, RecoveryCommandError> {
    if !matches!(relist.stage, "refunding" | "completed") {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    let completed_at = (relist.stage == "completed").then_some(now);
    let state_version = sqlx::query(
        r#"UPDATE public.settlement_recoveries
           SET stage = $1,
               relisted_auction_id = $2,
               relisted_lot_id = $3,
               next_attempt_at = NULL,
               completed_at = $4,
               state_version = $5,
               updated_at = $6
           WHERE id = $7
             AND stage = 'relist_pending'
             AND state_version = $8
             AND resolution = $9
             AND custody_proof_kind IS NOT NULL
             AND relisted_auction_id IS NULL
             AND relisted_lot_id IS NULL
             AND blocking_code IS NULL
             AND manual_reconcile_delivery_id IS NULL
           RETURNING state_version"#,
    )
    .bind(relist.stage)
    .bind(relisted_auction_id)
    .bind(relisted_lot_id)
    .bind(completed_at)
    .bind(next_state_version)
    .bind(now)
    .bind(recovery.id)
    .bind(recovery.state_version)
    .bind(relist.resolution)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::StaleState)?;
    state_version.try_get("state_version").map_err(map_sql_error)
}

#[cfg(feature = "postgres")]
async fn insert_worker_recovery_audit(
    transaction: &mut Transaction<'_, Postgres>,
    recovery_id: Uuid,
    action: &str,
    occurred_at: DateTime<Utc>,
    metadata: Value,
) -> Result<(), RecoveryCommandError> {
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
                result, request_id, source, metadata)
           VALUES ($1, $2, 'system', 'recovery-worker', $3, 'settlement_recovery',
                   $4, 'success', $5, 'worker', $6)"#,
    )
    .bind(Uuid::new_v4())
    .bind(occurred_at)
    .bind(action)
    .bind(recovery_id.to_string())
    .bind(None::<String>)
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
struct RecoveryScope {
    auction_id: Uuid,
    lot_id: Uuid,
    world_id: String,
    equipment_id: Uuid,
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
struct RecoveryRow {
    id: Uuid,
    auction_id: Uuid,
    lot_id: Uuid,
    world_id: String,
    equipment_id: Uuid,
    payment_obligation_id: Uuid,
    equipment_obligation_id: Option<Uuid>,
    seller_payout_obligation_id: Uuid,
    buyer_refund_obligation_id: Uuid,
    stage: String,
    resolution: Option<String>,
    state_version: i64,
    blocking_code: Option<String>,
    manual_reconcile_delivery_id: Option<Uuid>,
    relisted_auction_id: Option<Uuid>,
    relisted_lot_id: Option<Uuid>,
    custody_proof_kind: Option<String>,
    custody_proof_reference: Option<String>,
    custody_proof_digest: Option<Vec<u8>>,
    custody_proof_observed_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
struct OriginalLot {
    equipment_id: Uuid,
    submission_id: Option<Uuid>,
    status: String,
    start_price: i64,
    equipment_world_id: String,
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
struct ObligationRow {
    id: Uuid,
    auction_id: Uuid,
    lot_id: Uuid,
    world_id: String,
    kind: String,
    state: String,
    equipment_id: Uuid,
    amount: i64,
    recipient_user_id: Option<Uuid>,
    recipient_game_identity: Option<String>,
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
struct ActionRow {
    id: Uuid,
    kind: String,
    state: String,
    delivery_id: Option<Uuid>,
    proof_kind: Option<String>,
    proof_reference: Option<String>,
    proof_digest: Option<Vec<u8>>,
    proof_observed_at: Option<DateTime<Utc>>,
}

#[cfg(feature = "postgres")]
struct RelistResult {
    stage: &'static str,
    resolution: &'static str,
    auction_id: Option<Uuid>,
    lot_id: Option<Uuid>,
}

#[cfg(feature = "postgres")]
async fn load_replayed_command(
    transaction: &mut Transaction<'_, Postgres>,
    command: &RecoveryConfirmationCommand,
) -> Result<Option<RecoveryConfirmationResult>, RecoveryCommandError> {
    let Some(row) = sqlx::query(
        r#"SELECT recovery_id, expected_state_version, action, request_digest,
                  result_code, result_payload
           FROM public.settlement_recovery_commands
           WHERE command_id = $1"#,
    )
    .bind(command.command_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    else {
        return Ok(None);
    };

    let stored_recovery_id: Uuid = row.try_get("recovery_id").map_err(map_sql_error)?;
    let stored_version: i64 = row.try_get("expected_state_version").map_err(map_sql_error)?;
    let stored_action: String = row.try_get("action").map_err(map_sql_error)?;
    let stored_digest: Vec<u8> = row.try_get("request_digest").map_err(map_sql_error)?;
    if stored_recovery_id != command.recovery_id.as_uuid()
        || stored_version != command.expected_state_version
        || stored_action != command.action.as_str()
        || stored_digest != command.request_hash.as_bytes()
    {
        return Err(RecoveryCommandError::CommandConflict);
    }

    let result_code: String = row.try_get("result_code").map_err(map_sql_error)?;
    if result_code != "ok" {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    let payload: Value = row.try_get("result_payload").map_err(map_sql_error)?;
    let result: RecoveryConfirmationResult =
        serde_json::from_value(payload).map_err(|_| RecoveryCommandError::InvalidStoredResult)?;
    result.validate()?;
    if result.recovery_id != command.recovery_id || result.action != command.action {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    if result.state_version <= command.expected_state_version {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    Ok(Some(result))
}

#[cfg(feature = "postgres")]
async fn load_recovery_scope(
    transaction: &mut Transaction<'_, Postgres>,
    recovery_id: Uuid,
) -> Result<RecoveryScope, RecoveryCommandError> {
    let row = sqlx::query(
        r#"SELECT auction_id, lot_id, world_id, equipment_id
           FROM public.settlement_recoveries
           WHERE id = $1"#,
    )
    .bind(recovery_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::NotFound)?;
    Ok(RecoveryScope {
        auction_id: row.try_get("auction_id").map_err(map_sql_error)?,
        lot_id: row.try_get("lot_id").map_err(map_sql_error)?,
        world_id: row.try_get("world_id").map_err(map_sql_error)?,
        equipment_id: row.try_get("equipment_id").map_err(map_sql_error)?,
    })
}

#[cfg(feature = "postgres")]
async fn lock_world(
    transaction: &mut Transaction<'_, Postgres>,
    world_id: &str,
) -> Result<(), RecoveryCommandError> {
    let exists = sqlx::query("SELECT id FROM public.worlds WHERE id = $1 FOR UPDATE")
        .bind(world_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
        .is_some();
    if exists { Ok(()) } else { Err(RecoveryCommandError::NotFound) }
}

#[cfg(feature = "postgres")]
async fn lock_auction(
    transaction: &mut Transaction<'_, Postgres>,
    auction_id: Uuid,
) -> Result<AuctionLock, RecoveryCommandError> {
    let row = sqlx::query(
        "SELECT world_id, status, state_version FROM public.auctions WHERE id = $1 FOR UPDATE",
    )
    .bind(auction_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::NotFound)?;
    Ok(AuctionLock {
        world_id: row.try_get("world_id").map_err(map_sql_error)?,
        status: row.try_get("status").map_err(map_sql_error)?,
        state_version: row.try_get("state_version").map_err(map_sql_error)?,
    })
}

#[cfg(feature = "postgres")]
struct AuctionLock {
    world_id: String,
    status: String,
    #[allow(dead_code)]
    state_version: i64,
}

#[cfg(feature = "postgres")]
async fn lock_recovery(
    transaction: &mut Transaction<'_, Postgres>,
    recovery_id: Uuid,
) -> Result<RecoveryRow, RecoveryCommandError> {
    let row = sqlx::query(
        r#"SELECT id, auction_id, lot_id, world_id, equipment_id,
                  payment_obligation_id, equipment_obligation_id,
                  seller_payout_obligation_id, buyer_refund_obligation_id,
                  stage, resolution, state_version, blocking_code,
                  manual_reconcile_delivery_id, relisted_auction_id, relisted_lot_id,
                  custody_proof_kind, custody_proof_reference, custody_proof_digest,
                  custody_proof_observed_at, created_at
           FROM public.settlement_recoveries
           WHERE id = $1
           FOR UPDATE"#,
    )
    .bind(recovery_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::NotFound)?;
    Ok(RecoveryRow {
        id: row.try_get("id").map_err(map_sql_error)?,
        auction_id: row.try_get("auction_id").map_err(map_sql_error)?,
        lot_id: row.try_get("lot_id").map_err(map_sql_error)?,
        world_id: row.try_get("world_id").map_err(map_sql_error)?,
        equipment_id: row.try_get("equipment_id").map_err(map_sql_error)?,
        payment_obligation_id: row.try_get("payment_obligation_id").map_err(map_sql_error)?,
        equipment_obligation_id: row.try_get("equipment_obligation_id").map_err(map_sql_error)?,
        seller_payout_obligation_id: row
            .try_get("seller_payout_obligation_id")
            .map_err(map_sql_error)?,
        buyer_refund_obligation_id: row
            .try_get("buyer_refund_obligation_id")
            .map_err(map_sql_error)?,
        stage: row.try_get("stage").map_err(map_sql_error)?,
        resolution: row.try_get("resolution").map_err(map_sql_error)?,
        state_version: row.try_get("state_version").map_err(map_sql_error)?,
        blocking_code: row.try_get("blocking_code").map_err(map_sql_error)?,
        manual_reconcile_delivery_id: row
            .try_get("manual_reconcile_delivery_id")
            .map_err(map_sql_error)?,
        relisted_auction_id: row.try_get("relisted_auction_id").map_err(map_sql_error)?,
        relisted_lot_id: row.try_get("relisted_lot_id").map_err(map_sql_error)?,
        custody_proof_kind: row.try_get("custody_proof_kind").map_err(map_sql_error)?,
        custody_proof_reference: row.try_get("custody_proof_reference").map_err(map_sql_error)?,
        custody_proof_digest: row.try_get("custody_proof_digest").map_err(map_sql_error)?,
        custody_proof_observed_at: row
            .try_get("custody_proof_observed_at")
            .map_err(map_sql_error)?,
        created_at: row.try_get("created_at").map_err(map_sql_error)?,
    })
}

#[cfg(feature = "postgres")]
fn validate_recovery_row(
    recovery: &RecoveryRow,
    scope: &RecoveryScope,
    command: &RecoveryConfirmationCommand,
) -> Result<(), RecoveryCommandError> {
    if recovery.id != command.recovery_id.as_uuid()
        || recovery.auction_id != scope.auction_id
        || recovery.lot_id != scope.lot_id
        || recovery.world_id != scope.world_id
        || recovery.equipment_id != scope.equipment_id
    {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    if recovery.state_version != command.expected_state_version {
        return Err(RecoveryCommandError::StaleState);
    }
    if recovery.stage != "reclaiming"
        || recovery.resolution.is_some()
        || recovery.relisted_auction_id.is_some()
        || recovery.relisted_lot_id.is_some()
    {
        return Err(RecoveryCommandError::NotConfirmable);
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn lock_original_lot(
    transaction: &mut Transaction<'_, Postgres>,
    auction_id: Uuid,
    lot_id: Uuid,
    equipment_id: Uuid,
) -> Result<OriginalLot, RecoveryCommandError> {
    let row = sqlx::query(
        r#"SELECT lot.equipment_id, lot.submission_id, lot.status, lot.start_price,
                  equipment.world_id AS equipment_world_id
           FROM public.lots AS lot
           INNER JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
           WHERE lot.id = $1 AND lot.auction_id = $2 AND lot.equipment_id = $3
           FOR UPDATE OF lot, equipment"#,
    )
    .bind(lot_id)
    .bind(auction_id)
    .bind(equipment_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::NotFound)?;
    Ok(OriginalLot {
        equipment_id: row.try_get("equipment_id").map_err(map_sql_error)?,
        submission_id: row.try_get("submission_id").map_err(map_sql_error)?,
        status: row.try_get("status").map_err(map_sql_error)?,
        start_price: row.try_get("start_price").map_err(map_sql_error)?,
        equipment_world_id: row.try_get("equipment_world_id").map_err(map_sql_error)?,
    })
}

#[cfg(feature = "postgres")]
async fn lock_obligations(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
) -> Result<BTreeMap<Uuid, ObligationRow>, RecoveryCommandError> {
    let mut ids = vec![
        recovery.payment_obligation_id,
        recovery.seller_payout_obligation_id,
        recovery.buyer_refund_obligation_id,
    ];
    if let Some(id) = recovery.equipment_obligation_id {
        ids.push(id);
    }
    ids.sort_unstable();

    let mut obligations = BTreeMap::new();
    for id in ids {
        let row = sqlx::query(
            r#"SELECT id, auction_id, lot_id, world_id, obligation_kind, business_state,
                      equipment_id, amount, recipient_user_id, recipient_game_identity
               FROM public.settlement_obligations
               WHERE id = $1
               FOR UPDATE"#,
        )
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
        .ok_or(RecoveryCommandError::NotConfirmable)?;
        let obligation = ObligationRow {
            id: row.try_get("id").map_err(map_sql_error)?,
            auction_id: row.try_get("auction_id").map_err(map_sql_error)?,
            lot_id: row.try_get("lot_id").map_err(map_sql_error)?,
            world_id: row.try_get("world_id").map_err(map_sql_error)?,
            kind: row.try_get("obligation_kind").map_err(map_sql_error)?,
            state: row.try_get("business_state").map_err(map_sql_error)?,
            equipment_id: row.try_get("equipment_id").map_err(map_sql_error)?,
            amount: row.try_get("amount").map_err(map_sql_error)?,
            recipient_user_id: row.try_get("recipient_user_id").map_err(map_sql_error)?,
            recipient_game_identity: row
                .try_get("recipient_game_identity")
                .map_err(map_sql_error)?,
        };
        if obligation.id != id
            || obligation.auction_id != recovery.auction_id
            || obligation.lot_id != recovery.lot_id
            || obligation.world_id != recovery.world_id
            || obligation.equipment_id != recovery.equipment_id
        {
            return Err(RecoveryCommandError::InvalidStoredResult);
        }
        obligations.insert(id, obligation);
    }
    Ok(obligations)
}

#[cfg(feature = "postgres")]
async fn lock_actions(
    transaction: &mut Transaction<'_, Postgres>,
    recovery_id: Uuid,
) -> Result<Vec<ActionRow>, RecoveryCommandError> {
    let rows = sqlx::query(
        r#"SELECT id, kind, execution_state, delivery_id, proof_kind,
                  proof_reference, proof_digest, proof_observed_at
           FROM public.settlement_recovery_actions
           WHERE recovery_id = $1
           ORDER BY kind, id
           FOR UPDATE"#,
    )
    .bind(recovery_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    rows.into_iter()
        .map(|row| {
            Ok(ActionRow {
                id: row.try_get("id").map_err(map_sql_error)?,
                kind: row.try_get("kind").map_err(map_sql_error)?,
                state: row.try_get("execution_state").map_err(map_sql_error)?,
                delivery_id: row.try_get("delivery_id").map_err(map_sql_error)?,
                proof_kind: row.try_get("proof_kind").map_err(map_sql_error)?,
                proof_reference: row.try_get("proof_reference").map_err(map_sql_error)?,
                proof_digest: row.try_get("proof_digest").map_err(map_sql_error)?,
                proof_observed_at: row.try_get("proof_observed_at").map_err(map_sql_error)?,
            })
        })
        .collect()
}

#[cfg(feature = "postgres")]
fn validate_actions<'a>(
    recovery: &RecoveryRow,
    actions: &'a [ActionRow],
    requested: RecoveryConfirmationAction,
) -> Result<&'a ActionRow, RecoveryCommandError> {
    let expected_kind = confirmation_action_kind(&recovery.world_id, requested)
        .ok_or(RecoveryCommandError::NotConfirmable)?;
    let mut candidate = None;
    let mut blacklist_satisfied = false;
    let mut candidate_action = None;
    for action in actions {
        if action.kind == "buyer_blacklist" && action.state == "satisfied" {
            blacklist_satisfied = true;
        }
        if action.state != "satisfied" {
            continue;
        }
        let Some(candidate_value) = action_confirmation(&recovery.world_id, &action.kind) else {
            continue;
        };
        if candidate.is_some_and(|value| value != candidate_value) {
            return Err(RecoveryCommandError::NotConfirmable);
        }
        candidate = Some(candidate_value);
        candidate_action = Some(action);
    }

    let selected = actions
        .iter()
        .find(|action| action.kind == expected_kind)
        .ok_or(RecoveryCommandError::EvidenceUnresolved)?;
    if candidate != Some(requested)
        || selected.state != "satisfied"
        || selected.delivery_id.is_none()
        || selected.proof_kind.is_none()
        || selected.proof_reference.is_none()
        || selected.proof_digest.is_none()
        || selected.proof_observed_at.is_none()
        || candidate_action.map(|action| action.id) != Some(selected.id)
    {
        return Err(RecoveryCommandError::EvidenceUnresolved);
    }
    if matches!(
        requested,
        RecoveryConfirmationAction::Reclaimed | RecoveryConfirmationAction::Cancelled
    ) && !blacklist_satisfied
    {
        return Err(RecoveryCommandError::NotConfirmable);
    }
    Ok(selected)
}

#[cfg(feature = "postgres")]
fn validate_custody_evidence(
    recovery: &RecoveryRow,
    action: &ActionRow,
) -> Result<(), RecoveryCommandError> {
    if recovery.blocking_code.is_some()
        || recovery.manual_reconcile_delivery_id.is_some()
        || recovery.custody_proof_kind.as_deref() != Some("delivery_recovery_take_succeeded")
        || recovery.custody_proof_reference.is_none()
        || recovery.custody_proof_digest.is_none()
        || recovery.custody_proof_observed_at.is_none()
        || recovery.custody_proof_kind != action.proof_kind
        || recovery.custody_proof_reference != action.proof_reference
        || recovery.custody_proof_digest != action.proof_digest
        || recovery.custody_proof_observed_at != action.proof_observed_at
    {
        return Err(RecoveryCommandError::EvidenceUnresolved);
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn validate_obligations(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    obligations: &BTreeMap<Uuid, ObligationRow>,
    action: RecoveryConfirmationAction,
) -> Result<(), RecoveryCommandError> {
    let payment = obligations
        .get(&recovery.payment_obligation_id)
        .ok_or(RecoveryCommandError::NotConfirmable)?;
    let seller = obligations
        .get(&recovery.seller_payout_obligation_id)
        .ok_or(RecoveryCommandError::NotConfirmable)?;
    let refund = obligations
        .get(&recovery.buyer_refund_obligation_id)
        .ok_or(RecoveryCommandError::NotConfirmable)?;
    if seller.kind != "seller_payout"
        || seller.state != "blocked"
        || seller.amount <= 0
        || seller.recipient_game_identity.as_deref().is_none_or(str::is_empty)
        || refund.kind != "buyer_refund"
        || refund.state != "contingent"
        || refund.amount <= 0
        || refund.recipient_user_id.is_none()
    {
        return Err(RecoveryCommandError::NotConfirmable);
    }

    let payment_state = lock_fulfillment_state(transaction, payment.id).await?;
    match action {
        RecoveryConfirmationAction::Reclaimed => {
            if recovery.world_id != "main"
                || payment.kind != "winner_equipment_cod"
                || payment.amount <= 0
                || payment.recipient_user_id.is_none()
                || payment_state.as_deref() == Some("satisfied")
            {
                return Err(RecoveryCommandError::NotConfirmable);
            }
        }
        RecoveryConfirmationAction::Cancelled => {
            if recovery.world_id != "isekai"
                || payment.kind != "winner_payment_carrier_cod"
                || payment.amount <= 0
                || payment.recipient_user_id.is_none()
                || payment_state.as_deref() == Some("satisfied")
            {
                return Err(RecoveryCommandError::NotConfirmable);
            }
        }
        RecoveryConfirmationAction::EquipmentRecovered => {
            let equipment_id =
                recovery.equipment_obligation_id.ok_or(RecoveryCommandError::NotConfirmable)?;
            let equipment =
                obligations.get(&equipment_id).ok_or(RecoveryCommandError::NotConfirmable)?;
            let equipment_state = lock_fulfillment_state(transaction, equipment.id).await?;
            if recovery.world_id != "isekai"
                || payment.kind != "winner_payment_carrier_cod"
                || payment_state.as_deref() != Some("satisfied")
                || equipment.kind != "winner_equipment_handoff"
                || equipment.state != "blocked"
                || equipment_state.as_deref() == Some("satisfied")
            {
                return Err(RecoveryCommandError::NotConfirmable);
            }
        }
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn lock_fulfillment_state(
    transaction: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
) -> Result<Option<String>, RecoveryCommandError> {
    let row = sqlx::query(
        "SELECT execution_state FROM public.settlement_obligation_fulfillments
         WHERE obligation_id = $1 FOR UPDATE",
    )
    .bind(obligation_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    row.map(|row| row.try_get("execution_state").map_err(map_sql_error)).transpose()
}

#[cfg(feature = "postgres")]
async fn database_now(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<DateTime<Utc>, RecoveryCommandError> {
    sqlx::query_scalar("SELECT CURRENT_TIMESTAMP")
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql_error)
}

#[cfg(feature = "postgres")]
async fn activate_buyer_refund(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    now: DateTime<Utc>,
) -> Result<(), RecoveryCommandError> {
    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligations
           SET business_state = 'ready', state_version = state_version + 1, updated_at = $1
           WHERE id = $2 AND auction_id = $3 AND lot_id = $4
             AND obligation_kind = 'buyer_refund' AND business_state = 'contingent'
           RETURNING id"#,
    )
    .bind(now)
    .bind(recovery.buyer_refund_obligation_id)
    .bind(recovery.auction_id)
    .bind(recovery.lot_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if updated.is_none() {
        return Err(RecoveryCommandError::NotConfirmable);
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn relist_recovery(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    original_lot: &OriginalLot,
    command_id: CommandId,
    action: RecoveryConfirmationAction,
    now: DateTime<Utc>,
) -> Result<RelistResult, RecoveryCommandError> {
    let row = sqlx::query(
        r#"SELECT auction.id, auction.preview_revision, auction.state_version
           FROM public.auction_intake_targets AS target
           INNER JOIN public.auctions AS auction
               ON auction.id = target.auction_id
              AND auction.world_id = target.world_id
              AND auction.status = 'draft'
           WHERE target.world_id = $1 AND target.target_status = 'draft'
           FOR UPDATE OF target, auction"#,
    )
    .bind(&recovery.world_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let Some(target) = row else {
        return Ok(RelistResult {
            stage: "relist_pending",
            resolution: action.as_str(),
            auction_id: None,
            lot_id: None,
        });
    };

    let target_id: Uuid = target.try_get("id").map_err(map_sql_error)?;
    let preview_revision: i64 = target.try_get("preview_revision").map_err(map_sql_error)?;
    let target_state_version: i64 = target.try_get("state_version").map_err(map_sql_error)?;
    if preview_revision < 0 || target_state_version <= 0 || preview_revision == i64::MAX {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    let max_lot_number: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(lot_number), 0)::bigint FROM public.lots WHERE auction_id = $1",
    )
    .bind(target_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if max_lot_number >= i64::from(i32::MAX) {
        return Err(RecoveryCommandError::NotConfirmable);
    }

    let relisted_lot_id =
        deterministic_uuid(CommandId::from_uuid(recovery.id), "recovery-relist-lot", 1);
    sqlx::query(
        r#"INSERT INTO public.lots
               (id, auction_id, equipment_id, submission_id, lot_number, status,
                start_price, current_price, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, 'pending', $6, $6, $7, $7)"#,
    )
    .bind(relisted_lot_id)
    .bind(target_id)
    .bind(original_lot.equipment_id)
    .bind(original_lot.submission_id)
    .bind(max_lot_number + 1)
    .bind(original_lot.start_price)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;

    let next_revision = preview_revision + 1;
    let updated = sqlx::query(
        r#"UPDATE public.auctions
           SET preview_revision = $1, state_version = state_version + 1, updated_at = $2
           WHERE id = $3 AND preview_revision = $4 AND status IN ('draft', 'preview')
           RETURNING state_version"#,
    )
    .bind(next_revision)
    .bind(now)
    .bind(target_id)
    .bind(preview_revision)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::StaleState)?;
    let target_next_state_version: i64 = updated.try_get("state_version").map_err(map_sql_error)?;
    if target_next_state_version <= target_state_version {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }

    insert_preview_event(
        transaction,
        target_id,
        &recovery.world_id,
        next_revision,
        target_next_state_version,
        command_id,
    )
    .await?;

    let stage = if action == RecoveryConfirmationAction::EquipmentRecovered {
        "refunding"
    } else {
        "completed"
    };
    Ok(RelistResult {
        stage,
        resolution: action.as_str(),
        auction_id: Some(target_id),
        lot_id: Some(relisted_lot_id),
    })
}

#[cfg(feature = "postgres")]
async fn insert_preview_event(
    transaction: &mut Transaction<'_, Postgres>,
    auction_id: Uuid,
    world_id: &str,
    preview_revision: i64,
    state_version: i64,
    command_id: CommandId,
) -> Result<(), RecoveryCommandError> {
    let stream_key = format!("auction:{auction_id}");
    sqlx::query(
        "INSERT INTO public.realtime_streams (stream_key) VALUES ($1)
         ON CONFLICT (stream_key) DO NOTHING",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let row = sqlx::query(
        r#"UPDATE public.realtime_streams
           SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
           WHERE stream_key = $1
           RETURNING next_sequence - 1 AS stream_sequence"#,
    )
    .bind(&stream_key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let sequence: i64 = row.try_get("stream_sequence").map_err(map_sql_error)?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
               (event_id, stream_key, stream_sequence, kind, schema_version,
                public_payload, state_version, origin_command_id)
           VALUES ($1, $2, $3, 'preview_catalog_changed', 1, $4, $5, $6)"#,
    )
    .bind(deterministic_uuid(command_id, "recovery-relist-preview-event", 1))
    .bind(&stream_key)
    .bind(sequence)
    .bind(json!({
        "world": world_id,
        "auction_id": auction_id,
        "preview_revision": preview_revision,
    }))
    .bind(state_version)
    .bind(command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn update_recovery(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    relist: &RelistResult,
    next_state_version: i64,
    now: DateTime<Utc>,
) -> Result<i64, RecoveryCommandError> {
    let completed_at = (relist.stage == "completed").then_some(now);
    let next_attempt_at = if relist.stage == "relist_pending" {
        Some(now.checked_add_signed(ChronoDuration::minutes(1)).ok_or_else(|| {
            RecoveryCommandError::Failed("recovery relist 重试时间溢出".to_owned())
        })?)
    } else {
        None
    };
    let row = sqlx::query(
        r#"UPDATE public.settlement_recoveries
           SET stage = $1, resolution = $2, relisted_auction_id = $3, relisted_lot_id = $4,
               next_attempt_at = $5, completed_at = $6, state_version = $7, updated_at = $8
           WHERE id = $9 AND stage = 'reclaiming' AND resolution IS NULL
             AND state_version = $10 AND relisted_auction_id IS NULL AND relisted_lot_id IS NULL
           RETURNING state_version"#,
    )
    .bind(relist.stage)
    .bind(relist.resolution)
    .bind(relist.auction_id)
    .bind(relist.lot_id)
    .bind(next_attempt_at)
    .bind(completed_at)
    .bind(next_state_version)
    .bind(now)
    .bind(recovery.id)
    .bind(recovery.state_version)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::StaleState)?;
    row.try_get("state_version").map_err(map_sql_error)
}

#[cfg(feature = "postgres")]
#[cfg(feature = "postgres")]
async fn insert_recovery_command(
    transaction: &mut Transaction<'_, Postgres>,
    command: &RecoveryConfirmationCommand,
    result: &RecoveryConfirmationResult,
    now: DateTime<Utc>,
) -> Result<(), RecoveryCommandError> {
    let payload =
        serde_json::to_value(result).map_err(|_| RecoveryCommandError::InvalidStoredResult)?;
    sqlx::query(
        r#"INSERT INTO public.settlement_recovery_commands
               (command_id, recovery_id, expected_state_version, action, request_digest,
                result_code, result_payload, created_at)
           VALUES ($1, $2, $3, $4, $5, 'ok', $6, $7)"#,
    )
    .bind(command.command_id.as_uuid())
    .bind(command.recovery_id.as_uuid())
    .bind(command.expected_state_version)
    .bind(command.action.as_str())
    .bind(command.request_hash.as_bytes().to_vec())
    .bind(payload)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(map_insert_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_recovery_audit(
    transaction: &mut Transaction<'_, Postgres>,
    command: &RecoveryConfirmationCommand,
    recovery: &RecoveryRow,
    action: &ActionRow,
    result: &RecoveryConfirmationResult,
) -> Result<(), RecoveryCommandError> {
    let metadata = json!({
        "command_id": command.command_id.as_uuid(),
        "recovery_action_id": action.id,
        "delivery_id": action.delivery_id,
        "previous_stage": "reclaiming",
        "next_stage": result.stage,
        "proof_kind": action.proof_kind,
        "relisted_auction_id": result.relisted_auction_id,
        "relisted_lot_id": result.relisted_lot_id,
        "previous_state_version": command.expected_state_version,
        "state_version": result.state_version,
    });
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, actor_type, actor_id, action, entity_type, entity_id, result,
                request_id, source, metadata)
           VALUES ($1, $2, $3, $4, 'recovery', $5, 'success', $6, $7, $8)"#,
    )
    .bind(Uuid::new_v4())
    .bind(command.actor.actor_type.database_name())
    .bind(&command.actor.actor_id)
    .bind(format!("auction.recovery.{}", command.action.as_str()))
    .bind(recovery.id.to_string())
    .bind(command.command_id.to_string())
    .bind(command.actor.actor_type.database_name())
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
fn confirmation_action_kind(
    world_id: &str,
    action: RecoveryConfirmationAction,
) -> Option<&'static str> {
    match (world_id, action) {
        ("main", RecoveryConfirmationAction::Reclaimed) => Some("reclaim_equipment_take"),
        ("isekai", RecoveryConfirmationAction::Cancelled) => Some("cancel_carrier_take"),
        ("isekai", RecoveryConfirmationAction::EquipmentRecovered) => {
            Some("recover_equipment_take")
        }
        _ => None,
    }
}

#[cfg(feature = "postgres")]
fn action_confirmation(world_id: &str, kind: &str) -> Option<RecoveryConfirmationAction> {
    match (world_id, kind) {
        ("main", "reclaim_equipment_take") => Some(RecoveryConfirmationAction::Reclaimed),
        ("isekai", "cancel_carrier_take") => Some(RecoveryConfirmationAction::Cancelled),
        ("isekai", "recover_equipment_take") => {
            Some(RecoveryConfirmationAction::EquipmentRecovered)
        }
        _ => None,
    }
}

#[cfg(feature = "postgres")]
fn map_insert_error(error: sqlx::Error) -> RecoveryCommandError {
    if matches!(&error, sqlx::Error::Database(database) if database.code().as_deref() == Some("23505"))
    {
        RecoveryCommandError::CommandConflict
    } else {
        map_sql_error(error)
    }
}

#[cfg(feature = "postgres")]
fn map_sql_error(error: sqlx::Error) -> RecoveryCommandError {
    if matches!(&error, sqlx::Error::Database(database) if database.code().as_deref() == Some("40001"))
    {
        RecoveryCommandError::SerializationConflict
    } else {
        RecoveryCommandError::Failed("PostgreSQL recovery 确认事务失败".to_owned())
    }
}
