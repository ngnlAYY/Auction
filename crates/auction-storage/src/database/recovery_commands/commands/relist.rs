//! Recovery relist、预览事件、状态更新与命令记录。
//!
//! 该文件只承载 PostgreSQL recovery command 的单一职责，事务装配仍由父模块统一负责。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn relist_recovery(
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
pub(super) async fn insert_preview_event(
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
pub(super) async fn update_recovery(
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
pub(super) async fn insert_recovery_command(
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
pub(super) async fn insert_recovery_audit(
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
