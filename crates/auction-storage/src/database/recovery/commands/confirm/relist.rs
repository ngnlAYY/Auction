//! Worker recovery relist 的候选领取、证明复核和状态推进。

use {
    crate::database::recovery::commands::confirm::{
        block_recovery_relist, complete_recovery_relist, defer_recovery_relist,
        insert_worker_recovery_audit, scheduled_relist_command_id,
    },
    crate::database::recovery::commands::{
        database_now, is_conflict, lock_actions, lock_auction, lock_original_lot, lock_recovery,
        lock_world, map_sql_error, relist_recovery, validate_actions, validate_custody_evidence,
    },
    auction_application::recovery::{
        RecoveryCommandError, RecoveryConfirmationAction, RecoveryResolutionAdvance,
    },
    chrono::{DateTime, Duration as ChronoDuration, Utc},
    serde_json::json,
    sqlx::{Postgres, Row},
    uuid::Uuid,
};

pub(in crate::database::recovery::commands) async fn advance_relist_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
    retry_after: ChronoDuration,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError> {
    // 与旧 Worker 一致，在 Serializable runner 外生成一次事件 ID，并在重试间保持不变。
    let preview_event_id = Uuid::new_v4();
    crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::Recovery,
        crate::database::SERIALIZABLE_ATTEMPTS,
        is_conflict,
        || advance_relist_postgres_once(pool, now, retry_after, preview_event_id),
    )
    .await
}

pub(in crate::database::recovery::commands) async fn advance_relist_postgres_once(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
    retry_after: ChronoDuration,
    preview_event_id: Uuid,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError> {
    let mut transaction =
        pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await.map_err(map_sql_error)?;

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
    // 入参仅用于挑选已到期候选；重挂许可与新状态时间必须取所有权锁取得后的数据库实时时钟。
    // 已持久化的 action/custody 证明原样复核，不以此时钟改写外部收回事实。
    let now = database_now(&mut transaction).await?;

    let command_id = scheduled_relist_command_id(recovery.id);
    let relist = relist_recovery(
        &mut transaction,
        &recovery,
        &lot,
        command_id,
        preview_event_id,
        action,
        now,
    )
    .await?;
    if relist.auction_id.is_none() && relist.lot_id.is_none() {
        let next_state_version = if let Some(blocking_code) = relist.blocking_code {
            block_recovery_relist(
                &mut transaction,
                &recovery,
                relist.resolution,
                blocking_code,
                now,
            )
            .await?
        } else {
            let retry_at = now.checked_add_signed(retry_after).ok_or_else(|| {
                RecoveryCommandError::Failed("recovery relist 重试时间溢出".to_owned())
            })?;
            defer_recovery_relist(&mut transaction, &recovery, relist.resolution, retry_at, now)
                .await?
        };
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
