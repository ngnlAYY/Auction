//! Recovery relist、预览事件、状态更新与命令记录；事务装配由父模块统一负责。

use auction_application::recovery::{
    RecoveryCommandError, RecoveryConfirmationAction, RecoveryConfirmationCommand,
    RecoveryConfirmationResult,
};
use auction_domain::consignment::ListingEquipmentIdentity;
use auction_domain::legacy_natural_key_uuid;
use auction_time::{TimeDelta, Timestamp};
use serde_json::json;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::mapping::map_insert_error;
use super::mapping::map_sql_error;
use super::scope::ActionRow;
use super::scope::OriginalLot;
use super::scope::RecoveryRow;
use super::scope::RelistResult;
use crate::database::TrackedTransaction as Transaction;
use crate::database::operations::audit_health::AuditWriteResult;
use crate::database::pool::TrackedPool;
use crate::database::realtime::SequencedEvent;
use crate::database::realtime::append_sequenced_event;
use crate::database::realtime::preview_catalog_changed_payload;

/// 旧赛季装备在赛季开始后不能重挂的阻断码；Admin 报表经防腐层显示为 `recovery_relist_unavailable`。
const RELIST_SEASON_CLOSED: &str = "relist_season_closed";
/// 实物正被寄售活动预约或代管持有时暂不重列；与赛季关闭一样退出自动重试。
const RELIST_EQUIPMENT_HELD: &str = "relist_equipment_held";

/// 重列事务开始前预读的实物身份；锁内只复核身份，不把预读当作授权。
#[derive(Debug, Clone)]
pub(super) struct PreloadedRecoveryEquipment {
    pub(super) identity: Option<ListingEquipmentIdentity>,
    /// 预读时该实物是否已被寄售预约或 position 持有；只有为真时才需要实物租约。
    pub(super) equipment_held: bool,
}

pub(super) async fn relist_recovery(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    original_lot: &OriginalLot,
    preview_event_id: Uuid,
    action: RecoveryConfirmationAction,
    now: Timestamp,
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
    // 不变量 2、6：同一 `(world, 规范 EID)` 的寄售活动预约或代管 position 与拍卖建 lot
    // 共用实物互斥行。锁内显式查持有者，被持有时暂不重列，零新 lot；阻断码与赛季关闭
    // 一样退出自动重试，交由 Admin 处置寄售侧后再重列。
    if consignment_claim_schema_available(transaction).await.map_err(map_sql_error)? {
        let equipment_held: bool = sqlx::query_scalar(
            r#"SELECT EXISTS (
                   SELECT 1
                   FROM public.equipment AS equipment
                   INNER JOIN public.consignment_equipment_claims AS claim
                       ON claim.world_id = equipment.world_id
                      AND claim.canonical_eid =
                          public.consignment_eid_alias(equipment.external_equipment_id)
                   WHERE equipment.id = $1
                     AND (claim.reservation_id IS NOT NULL OR claim.position_id IS NOT NULL)
               )"#,
        )
        .bind(original_lot.equipment_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        if equipment_held {
            return Ok(RelistResult {
                stage: "relist_pending",
                resolution: action.as_str(),
                auction_id: None,
                lot_id: None,
                blocking_code: Some(RELIST_EQUIPMENT_HELD),
            });
        }
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

/// 按恢复记录预读实物身份；行不存在时返回 `None`，由事务保持既有 `NotFound` 语义。
pub(super) async fn preload_recovery_equipment(
    pool: &TrackedPool<Postgres>,
    recovery_id: Uuid,
) -> Result<Option<PreloadedRecoveryEquipment>, RecoveryCommandError> {
    let mut connection = pool.acquire().await.map_err(map_sql_error)?;
    if !consignment_claim_schema_available(&mut connection).await.map_err(map_sql_error)? {
        return Ok(None);
    }
    let row = sqlx::query(
        r#"SELECT recovery.world_id,
                  public.consignment_eid_alias(equipment.external_equipment_id) AS canonical_eid,
                  EXISTS (
                      SELECT 1
                      FROM public.consignment_equipment_claims AS claim
                      WHERE claim.world_id = recovery.world_id
                        AND claim.canonical_eid = public.consignment_eid_alias(
                            equipment.external_equipment_id
                        )
                        AND (claim.reservation_id IS NOT NULL
                             OR claim.position_id IS NOT NULL)
                  ) AS equipment_held
           FROM public.settlement_recoveries AS recovery
           INNER JOIN public.equipment AS equipment
               ON equipment.id = recovery.equipment_id
           WHERE recovery.id = $1"#,
    )
    .bind(recovery_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    row.map(|row| {
        let world_id: String = row.try_get("world_id").map_err(map_sql_error)?;
        let canonical_eid: Option<String> = row.try_get("canonical_eid").map_err(map_sql_error)?;
        let equipment_held: bool = row.try_get("equipment_held").map_err(map_sql_error)?;
        Ok(PreloadedRecoveryEquipment {
            identity: canonical_lock_identity(&world_id, canonical_eid),
            equipment_held,
        })
    })
    .transpose()
}

/// 按现有候选顺序预读一条到期重列的实物身份；真正领取仍由事务锁内重读决定。
pub(super) async fn preload_due_relist_equipment(
    pool: &TrackedPool<Postgres>,
    now: Timestamp,
) -> Result<Option<PreloadedRecoveryEquipment>, RecoveryCommandError> {
    let mut connection = pool.acquire().await.map_err(map_sql_error)?;
    if !consignment_claim_schema_available(&mut connection).await.map_err(map_sql_error)? {
        return Ok(None);
    }
    let row = sqlx::query(
        r#"SELECT recovery.world_id,
                  public.consignment_eid_alias(equipment.external_equipment_id) AS canonical_eid,
                  EXISTS (
                      SELECT 1
                      FROM public.consignment_equipment_claims AS claim
                      WHERE claim.world_id = recovery.world_id
                        AND claim.canonical_eid = public.consignment_eid_alias(
                            equipment.external_equipment_id
                        )
                        AND (claim.reservation_id IS NOT NULL
                             OR claim.position_id IS NOT NULL)
                  ) AS equipment_held
           FROM public.settlement_recoveries AS recovery
           INNER JOIN public.equipment AS equipment
               ON equipment.id = recovery.equipment_id
           WHERE recovery.stage = 'relist_pending'
             AND recovery.resolution IS NOT NULL
             AND recovery.blocking_code IS NULL
             AND recovery.manual_reconcile_delivery_id IS NULL
             AND recovery.custody_proof_kind IS NOT NULL
             AND recovery.relisted_auction_id IS NULL
             AND recovery.relisted_lot_id IS NULL
             AND (recovery.next_attempt_at IS NULL OR recovery.next_attempt_at <= $1)
           ORDER BY COALESCE(recovery.next_attempt_at, recovery.created_at),
                    recovery.created_at, recovery.id
           LIMIT 1"#,
    )
    .bind(now)
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    row.map(|row| {
        let world_id: String = row.try_get("world_id").map_err(map_sql_error)?;
        let canonical_eid: Option<String> = row.try_get("canonical_eid").map_err(map_sql_error)?;
        let equipment_held: bool = row.try_get("equipment_held").map_err(map_sql_error)?;
        Ok(PreloadedRecoveryEquipment {
            identity: canonical_lock_identity(&world_id, canonical_eid),
            equipment_held,
        })
    })
    .transpose()
}

/// 事务内锁定恢复记录后按同一别名规则重读实物身份。
pub(super) async fn recovery_equipment_identity(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
) -> Result<Option<ListingEquipmentIdentity>, RecoveryCommandError> {
    if !consignment_claim_schema_available(transaction).await.map_err(map_sql_error)? {
        return Ok(None);
    }
    let canonical_eid: Option<String> = sqlx::query_scalar(
        "SELECT public.consignment_eid_alias(external_equipment_id)
         FROM public.equipment WHERE id = $1",
    )
    .bind(recovery.equipment_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(canonical_lock_identity(&recovery.world_id, canonical_eid))
}

/// 预读集合变化时退出本事务重新规划；调用方按可重试冲突处理，零事实。
pub(super) fn ensure_preloaded_equipment(
    preloaded: Option<&PreloadedRecoveryEquipment>,
    current: Option<&ListingEquipmentIdentity>,
) -> Result<(), RecoveryCommandError> {
    let expected = preloaded.and_then(|preloaded| preloaded.identity.as_ref());
    if expected == current { Ok(()) } else { Err(RecoveryCommandError::SerializationConflict) }
}

fn canonical_lock_identity(
    world_id: &str,
    canonical_eid: Option<String>,
) -> Option<ListingEquipmentIdentity> {
    canonical_eid.and_then(|eid| ListingEquipmentIdentity::new(world_id, &eid).ok())
}

/// 迁移测试会在 000134 之前的库上运行当前二进制；缺少寄售实物表/别名函数时跳过新增判定，
/// 保持旧库上的既有重列行为。000134 之后两者同时存在，检查必须生效。
async fn consignment_claim_schema_available(
    connection: &mut sqlx::PgConnection,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT to_regclass('public.consignment_equipment_claims') IS NOT NULL
            AND to_regprocedure('public.consignment_eid_alias(text)') IS NOT NULL",
    )
    .fetch_one(connection)
    .await
}

pub(super) async fn insert_preview_event(
    transaction: &mut Transaction<'_, Postgres>,
    auction_id: Uuid,
    world_id: &str,
    preview_revision: i64,
    state_version: i64,
    event_id: Uuid,
) -> Result<(), RecoveryCommandError> {
    let stream_key = format!("auction:{auction_id}");
    append_sequenced_event(
        transaction,
        SequencedEvent {
            event_id,
            stream_key: &stream_key,
            kind: "preview_catalog_changed",
            schema_version: 1,
            payload: preview_catalog_changed_payload(world_id, auction_id, preview_revision),
            state_version,
            origin_command_id: None,
            created_at: None,
        },
    )
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

pub(super) async fn update_recovery(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    relist: &RelistResult,
    next_state_version: i64,
    now: Timestamp,
) -> Result<i64, RecoveryCommandError> {
    let completed_at = (relist.stage == "completed").then_some(now);
    // 永久阻断的重挂不再安排自动重试，等待 Admin 按阻断码处理。
    let next_attempt_at = if relist.stage == "relist_pending" && relist.blocking_code.is_none() {
        Some(now.checked_add_signed(TimeDelta::minutes(1)).ok_or_else(|| {
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
    now: Timestamp,
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

#[cfg(test)]
mod tests {
    use auction_application::recovery::RecoveryCommandError;
    use auction_domain::consignment::ListingEquipmentIdentity;

    use super::{PreloadedRecoveryEquipment, ensure_preloaded_equipment};

    #[test]
    fn preloaded_identity_mismatch_is_a_retryable_conflict() {
        let main_42 = ListingEquipmentIdentity::new("main", "42").expect("已知装备身份");
        let main_43 = ListingEquipmentIdentity::new("main", "43").expect("已知装备身份");
        let preloaded =
            PreloadedRecoveryEquipment { identity: Some(main_42.clone()), equipment_held: false };
        assert!(ensure_preloaded_equipment(Some(&preloaded), Some(&main_42)).is_ok());
        assert!(matches!(
            ensure_preloaded_equipment(Some(&preloaded), Some(&main_43)),
            Err(RecoveryCommandError::SerializationConflict)
        ));
        assert!(matches!(
            ensure_preloaded_equipment(None, Some(&main_43)),
            Err(RecoveryCommandError::SerializationConflict)
        ));
    }
}
