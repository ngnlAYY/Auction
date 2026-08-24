//! Recovery 确认、调度推进与命令审计。
//!
//! 该文件只承载 PostgreSQL recovery command 的单一职责，事务装配仍由父模块统一负责。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn confirm_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
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
pub(super) async fn confirm_postgres_once(
    pool: &crate::database::TrackedPool<Postgres>,
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
pub(super) async fn advance_relist_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
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
pub(super) async fn advance_relist_postgres_once(
    pool: &crate::database::TrackedPool<Postgres>,
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
pub(super) async fn complete_refund_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
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
pub(super) async fn complete_refund_postgres_once(
    pool: &crate::database::TrackedPool<Postgres>,
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
pub(super) fn scheduled_relist_command_id(recovery_id: Uuid) -> CommandId {
    CommandId::from_uuid(deterministic_uuid(
        CommandId::from_uuid(recovery_id),
        "recovery-relist-command",
        1,
    ))
}

#[cfg(feature = "postgres")]
pub(super) async fn defer_recovery_relist(
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
pub(super) async fn complete_recovery_relist(
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
pub(super) async fn insert_worker_recovery_audit(
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
