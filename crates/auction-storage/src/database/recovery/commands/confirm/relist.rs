//! Worker recovery relist 的候选领取、证明复核和状态推进。

use auction_application::core::CompetitionTransactionError;
use auction_application::recovery::{
    RecoveryCommandError, RecoveryConfirmationAction, RecoveryResolutionAdvance,
};
use auction_time::{TimeDelta, Timestamp};
use serde_json::json;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::database::consignment::commands::EquipmentLease;
use crate::database::recovery::commands::confirm::{
    block_recovery_relist, complete_recovery_relist, defer_recovery_relist,
    insert_worker_recovery_audit, scheduled_relist_command_id,
};
use crate::database::recovery::commands::mapping::is_conflict;
use crate::database::recovery::commands::mapping::map_sql_error;
use crate::database::recovery::commands::relist::ensure_preloaded_equipment;
use crate::database::recovery::commands::relist::preload_due_relist_equipment;
use crate::database::recovery::commands::relist::recovery_equipment_identity;
use crate::database::recovery::commands::relist::relist_recovery;
use crate::database::recovery::commands::scope::database_now;
use crate::database::recovery::commands::scope::lock_actions;
use crate::database::recovery::commands::scope::lock_auction;
use crate::database::recovery::commands::scope::lock_original_lot;
use crate::database::recovery::commands::scope::lock_recovery;
use crate::database::recovery::commands::scope::lock_world;
use crate::database::recovery::commands::scope::validate_actions;
use crate::database::recovery::commands::scope::validate_custody_evidence;

pub(in crate::database::recovery::commands) async fn advance_relist_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    now: Timestamp,
    retry_after: TimeDelta,
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
    now: Timestamp,
    retry_after: TimeDelta,
    preview_event_id: Uuid,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError> {
    advance_relist_postgres_once_between(pool, now, retry_after, preview_event_id, || async {})
        .await
}

/// 预读与实物租约之间保留一个测试钩子；生产入口传入空 future。
async fn advance_relist_postgres_once_between<F, Fut>(
    pool: &crate::database::TrackedPool<Postgres>,
    now: Timestamp,
    retry_after: TimeDelta,
    preview_event_id: Uuid,
    between: F,
) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    // 不变量 2：先按到期候选预读实物身份并取会话锁，再开 Serializable 快照。
    // 维护者 2026-11-23 决定：只有预读时该实物已被寄售预约或 position 持有才取实物租约；
    // 无持有时恢复重列保持既有路径（零租约、零等待）。预判之后才签发的预约由事务内强占用
    // 检查、Serializable 与 lot 守卫兜底；预读身份仍在锁内复核，变化按可重试冲突退出。
    let preloaded = preload_due_relist_equipment(pool, now).await?;
    between().await;
    let equipment = preloaded
        .as_ref()
        .filter(|preloaded| preloaded.equipment_held)
        .and_then(|preloaded| preloaded.identity.clone())
        .into_iter()
        .collect::<Vec<_>>();
    let lease = if equipment.is_empty() {
        None
    } else {
        Some(EquipmentLease::acquire(pool, &equipment).await.map_err(map_equipment_lease_error)?)
    };
    let result = advance_relist_postgres_once_locked(
        pool,
        now,
        retry_after,
        preview_event_id,
        preloaded.as_ref(),
    )
    .await;
    if let Some(lease) = lease {
        lease.release().await;
    }
    result
}

#[cfg(feature = "test-db")]
#[doc(hidden)]
impl crate::database::DatabaseStore {
    /// 测试两段执行：预读之后、取租约与锁内复核之前运行钩子，用于确定性地改变实物身份。
    pub async fn test_advance_relist_postgres_once_with_between_hook<F, Fut>(
        &self,
        now: Timestamp,
        retry_after: TimeDelta,
        preview_event_id: Uuid,
        between: F,
    ) -> Result<Option<RecoveryResolutionAdvance>, RecoveryCommandError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        advance_relist_postgres_once_between(
            self.pool(),
            now,
            retry_after,
            preview_event_id,
            between,
        )
        .await
    }
}

async fn advance_relist_postgres_once_locked(
    pool: &crate::database::TrackedPool<Postgres>,
    now: Timestamp,
    retry_after: TimeDelta,
    preview_event_id: Uuid,
    preloaded: Option<&crate::database::recovery::commands::relist::PreloadedRecoveryEquipment>,
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
    let current_equipment = recovery_equipment_identity(&mut transaction, &recovery).await?;
    ensure_preloaded_equipment(preloaded, current_equipment.as_ref())?;
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

    let relist =
        relist_recovery(&mut transaction, &recovery, &lot, preview_event_id, action, now).await?;
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
    }))
}

fn map_equipment_lease_error(error: CompetitionTransactionError) -> RecoveryCommandError {
    match error {
        CompetitionTransactionError::EquipmentLockUnavailable => {
            RecoveryCommandError::Failed("recovery 实物锁暂不可用".to_owned())
        }
        CompetitionTransactionError::CommandLedger(error) => {
            RecoveryCommandError::Failed(format!("recovery 实物锁账本失败: {error}"))
        }
        _ => RecoveryCommandError::Failed("recovery 实物锁取得失败".to_owned()),
    }
}
