//! Recovery relist、预览事件、状态更新与命令记录；事务装配由父模块统一负责。

use super::mapping::map_insert_error;
use super::mapping::map_sql_error;
use super::scope::ActionRow;
use super::scope::OriginalLot;
use super::scope::RecoveryRow;
use super::scope::RelistResult;
use crate::database::TrackedTransaction as Transaction;
use crate::database::operations::audit_health::AuditWriteResult;
use auction_application::recovery::{
    RecoveryCommandError, RecoveryConfirmationAction, RecoveryConfirmationCommand,
    RecoveryConfirmationResult,
};
use auction_domain::{CommandId, legacy_natural_key_uuid};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::json;
use sqlx::{Postgres, Row};
use uuid::Uuid;

/// 旧赛季装备在赛季开始后不能重挂的阻断码；Admin 报表经防腐层显示为 `recovery_relist_unavailable`。
const RELIST_SEASON_CLOSED: &str = "relist_season_closed";

pub(super) async fn relist_recovery(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    original_lot: &OriginalLot,
    command_id: CommandId,
    preview_event_id: Uuid,
    action: RecoveryConfirmationAction,
    now: DateTime<Utc>,
) -> Result<RelistResult, RecoveryCommandError> {
    // 原始收件或成交场次开拍后已跨过赛季边界的装备永不重挂。
    // 收件事实不可变，避免普通场次被改期到新赛季后把旧装备误认成新赛季资产。
    let season_closed: bool = sqlx::query_scalar(
        r#"SELECT EXISTS (
               SELECT 1
               FROM public.auctions AS boundary
               INNER JOIN public.auctions AS sold ON sold.id = $2
               INNER JOIN public.auction_submissions AS submission ON submission.id = $4
               WHERE boundary.world_id = $1
                 AND boundary.intake_opens_at
                     > LEAST(sold.scheduled_open_at, submission.received_at)
                 AND boundary.intake_opens_at <= $3
           )"#,
    )
    .bind(&recovery.world_id)
    .bind(recovery.auction_id)
    .bind(now)
    .bind(original_lot.submission_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if season_closed {
        return Ok(RelistResult {
            stage: "relist_pending",
            resolution: action.as_str(),
            auction_id: None,
            lot_id: None,
            blocking_code: Some(RELIST_SEASON_CLOSED),
        });
    }
    // 赛季开始前的新赛季草稿不接收装备，按“暂无可重挂目标”保留持久化退避。
    let row = sqlx::query(
        r#"SELECT auction.id, auction.preview_revision, auction.state_version
           FROM public.auction_intake_targets AS target
           INNER JOIN public.auctions AS auction
               ON auction.id = target.auction_id
              AND auction.world_id = target.world_id
              AND auction.status = 'draft'
           WHERE target.world_id = $1 AND target.target_status = 'draft'
             AND (auction.intake_opens_at IS NULL OR auction.intake_opens_at <= $2)
           FOR UPDATE OF target, auction"#,
    )
    .bind(&recovery.world_id)
    .bind(now)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let Some(target) = row else {
        return Ok(RelistResult {
            stage: "relist_pending",
            resolution: action.as_str(),
            auction_id: None,
            lot_id: None,
            blocking_code: None,
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
    let max_category_number: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(category_number), 0)::bigint
         FROM public.lots
         WHERE auction_id = $1 AND category_key = $2",
    )
    .bind(target_id)
    .bind(&original_lot.equipment_category)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if max_category_number >= i64::from(i32::MAX) {
        return Err(RecoveryCommandError::NotConfirmable);
    }
    let next_lot_number =
        max_lot_number.checked_add(1).ok_or(RecoveryCommandError::InvalidStoredResult)?;
    let next_category_number =
        max_category_number.checked_add(1).ok_or(RecoveryCommandError::InvalidStoredResult)?;

    let nil_uuid = Uuid::nil();
    let relisted_lot_id = legacy_natural_key_uuid(
        "auction.recovery.relist-lot.v1",
        &[recovery.id.as_bytes(), nil_uuid.as_bytes()],
    );
    sqlx::query(
        r#"INSERT INTO public.lots
               (id, auction_id, equipment_id, submission_id, lot_number,
                category_key, category_number, status, start_price, current_price,
                created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending', $8, $8, $9, $9)"#,
    )
    .bind(relisted_lot_id)
    .bind(target_id)
    .bind(original_lot.equipment_id)
    .bind(original_lot.submission_id)
    .bind(next_lot_number)
    .bind(&original_lot.equipment_category)
    .bind(next_category_number)
    .bind(original_lot.start_price)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;

    let next_revision =
        preview_revision.checked_add(1).ok_or(RecoveryCommandError::InvalidStoredResult)?;
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
        preview_event_id,
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
        blocking_code: None,
    })
}

pub(super) async fn insert_preview_event(
    transaction: &mut Transaction<'_, Postgres>,
    auction_id: Uuid,
    world_id: &str,
    preview_revision: i64,
    state_version: i64,
    command_id: CommandId,
    event_id: Uuid,
) -> Result<(), RecoveryCommandError> {
    let stream_key = format!("auction:{auction_id}");
    let sequence =
        crate::database::realtime::allocate_stream_sequence(transaction, &stream_key, None)
            .await
            .map_err(map_sql_error)?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
               (event_id, stream_key, stream_sequence, kind, schema_version,
                public_payload, state_version, origin_command_id)
           VALUES ($1, $2, $3, 'preview_catalog_changed', 1, $4, $5, $6)"#,
    )
    .bind(event_id)
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

pub(super) async fn update_recovery(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    relist: &RelistResult,
    next_state_version: i64,
    now: DateTime<Utc>,
) -> Result<i64, RecoveryCommandError> {
    let completed_at = (relist.stage == "completed").then_some(now);
    // 永久阻断的重挂不再安排自动重试，等待 Admin 按阻断码处理。
    let next_attempt_at = if relist.stage == "relist_pending" && relist.blocking_code.is_none() {
        Some(now.checked_add_signed(ChronoDuration::minutes(1)).ok_or_else(|| {
            RecoveryCommandError::Failed("recovery relist 重试时间溢出".to_owned())
        })?)
    } else {
        None
    };
    let row = sqlx::query(
        r#"UPDATE public.settlement_recoveries
           SET stage = $1, resolution = $2, relisted_auction_id = $3, relisted_lot_id = $4,
               next_attempt_at = $5, completed_at = $6, state_version = $7, updated_at = $8,
               blocking_code = COALESCE($11, blocking_code)
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
    .bind(relist.blocking_code)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::StaleState)?;
    row.try_get("state_version").map_err(map_sql_error)
}

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
    .bind(command.request_hash.as_bytes().as_slice())
    .bind(payload)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(map_insert_error)?;
    Ok(())
}

pub(super) async fn insert_recovery_audit(
    transaction: &mut Transaction<'_, Postgres>,
    command: &RecoveryConfirmationCommand,
    recovery: &RecoveryRow,
    action: &ActionRow,
    result: &RecoveryConfirmationResult,
) -> Result<(), RecoveryCommandError> {
    let mut metadata = json!({
        "command_id": command.command_id.as_uuid(),
        "recovery_action_id": action.id,
        "delivery_id": action.delivery_id,
        "previous_stage": recovery.stage,
        "next_stage": result.stage,
        "proof_kind": action.proof_kind,
        "relisted_auction_id": result.relisted_auction_id,
        "relisted_lot_id": result.relisted_lot_id,
        "previous_state_version": command.expected_state_version,
        "state_version": result.state_version,
    });
    if let Some(request_id) = command.request_id.as_ref() {
        let Some(object) = metadata.as_object_mut() else {
            return Err(RecoveryCommandError::InvalidStoredResult);
        };
        object.insert("request_id".to_owned(), json!(request_id));
    }
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, actor_type, actor_id, action, entity_type, entity_id, result,
                request_id, source, metadata)
           VALUES ($1, $2, $3, $4, 'auction', $5, 'success', $6, $7, $8)"#,
    )
    .bind(Uuid::new_v4())
    .bind(command.actor.actor_type.database_name())
    .bind(&command.actor.actor_id)
    .bind(format!("auction.recovery.{}", command.action.as_str()))
    .bind(recovery.id.to_string())
    .bind(None::<String>)
    .bind("admin")
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}
