//! Admin recovery 确认命令及其 Serializable 重试。

use auction_application::core::CompetitionTransactionError;
use auction_application::recovery::{
    RecoveryCommandError, RecoveryConfirmationAction, RecoveryConfirmationCommand,
    RecoveryConfirmationResult,
};
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::consignment::commands::EquipmentLease;
use crate::database::recovery::commands::confirm::closure::close_voided_fulfillments;
use crate::database::recovery::commands::confirm::recovered::close_recovered_dependents;
use crate::database::recovery::commands::mapping::is_conflict;
use crate::database::recovery::commands::mapping::map_reversal_error;
use crate::database::recovery::commands::mapping::map_sql_error;
use crate::database::recovery::commands::relist::ensure_preloaded_equipment;
use crate::database::recovery::commands::relist::insert_recovery_audit;
use crate::database::recovery::commands::relist::insert_recovery_command;
use crate::database::recovery::commands::relist::preload_recovery_equipment;
use crate::database::recovery::commands::relist::recovery_equipment_identity;
use crate::database::recovery::commands::relist::relist_recovery;
use crate::database::recovery::commands::relist::update_recovery;
use crate::database::recovery::commands::scope::activate_buyer_refund;
use crate::database::recovery::commands::scope::database_now;
use crate::database::recovery::commands::scope::load_recovery_scope;
use crate::database::recovery::commands::scope::load_replayed_command;
use crate::database::recovery::commands::scope::lock_actions;
use crate::database::recovery::commands::scope::lock_auction;
use crate::database::recovery::commands::scope::lock_obligation_deliveries;
use crate::database::recovery::commands::scope::lock_obligations;
use crate::database::recovery::commands::scope::lock_original_lot;
use crate::database::recovery::commands::scope::lock_recovery;
use crate::database::recovery::commands::scope::lock_world;
use crate::database::recovery::commands::scope::validate_actions;
use crate::database::recovery::commands::scope::validate_custody_evidence;
use crate::database::recovery::commands::scope::validate_obligations;
use crate::database::recovery::commands::scope::validate_recovery_row;

pub(in crate::database::recovery::commands) async fn confirm_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    command: RecoveryConfirmationCommand,
) -> Result<RecoveryConfirmationResult, RecoveryCommandError> {
    // 旧版在 Serializable runner 外生成事件 ID；同一次请求的三次事务尝试必须复用它。
    let preview_event_id = Uuid::new_v4();
    crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::Recovery,
        crate::database::SERIALIZABLE_ATTEMPTS,
        is_conflict,
        || confirm_postgres_once(pool, command.clone(), preview_event_id),
    )
    .await
}

pub(in crate::database::recovery::commands) async fn confirm_postgres_once(
    pool: &crate::database::TrackedPool<Postgres>,
    command: RecoveryConfirmationCommand,
    preview_event_id: Uuid,
) -> Result<RecoveryConfirmationResult, RecoveryCommandError> {
    confirm_postgres_once_between(pool, command, preview_event_id, || async {}).await
}

/// 预读与实物租约之间保留一个测试钩子；生产入口传入空 future。
async fn confirm_postgres_once_between<F, Fut>(
    pool: &crate::database::TrackedPool<Postgres>,
    command: RecoveryConfirmationCommand,
    preview_event_id: Uuid,
    between: F,
) -> Result<RecoveryConfirmationResult, RecoveryCommandError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    // 不变量 2：实物锁必须早于 Serializable 快照；预读只用于确定锁键，锁内仍重读身份。
    // 维护者 2026-11-23 决定：只有预读时该实物已被寄售预约或 position 持有才取实物租约；
    // 无持有时恢复重列保持既有路径（零租约、零等待）。预判之后才签发的预约由事务内强占用
    // 检查、Serializable 与 lot 守卫兜底；预读身份仍在锁内复核，变化按可重试冲突退出。
    let preloaded = preload_recovery_equipment(pool, command.recovery_id.as_uuid()).await?;
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
    let result =
        confirm_postgres_once_locked(pool, command, preview_event_id, preloaded.as_ref()).await;
    if let Some(lease) = lease {
        lease.release().await;
    }
    result
}

#[cfg(feature = "test-db")]
#[doc(hidden)]
impl crate::database::DatabaseStore {
    /// 测试两段执行：预读之后、取租约与锁内复核之前运行钩子，用于确定性地改变实物身份。
    pub async fn test_confirm_postgres_once_with_between_hook<F, Fut>(
        &self,
        command: RecoveryConfirmationCommand,
        preview_event_id: Uuid,
        between: F,
    ) -> Result<RecoveryConfirmationResult, RecoveryCommandError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        confirm_postgres_once_between(self.pool(), command, preview_event_id, between).await
    }
}

async fn confirm_postgres_once_locked(
    pool: &crate::database::TrackedPool<Postgres>,
    command: RecoveryConfirmationCommand,
    preview_event_id: Uuid,
    preloaded: Option<&crate::database::recovery::commands::relist::PreloadedRecoveryEquipment>,
) -> Result<RecoveryConfirmationResult, RecoveryCommandError> {
    let mut transaction =
        pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await.map_err(map_sql_error)?;

    if let Some(result) = load_replayed_command(&mut transaction, &command).await? {
        transaction.commit().await.map_err(map_sql_error)?;
        return Ok(result);
    }

    let scope = load_recovery_scope(&mut transaction, command.recovery_id.as_uuid()).await?;
    // 确认必然落定 resolution 并冲正补偿池；先锁池再锁世界，与结算的锁顺序一致。
    crate::database::settlement::compensation_reversal::lock_pool(&mut transaction)
        .await
        .map_err(map_reversal_error)?;
    lock_world(&mut transaction, &scope.world_id).await?;
    let auction = lock_auction(&mut transaction, scope.auction_id).await?;
    if auction.world_id != scope.world_id || auction.status != "settled" {
        return Err(RecoveryCommandError::NotConfirmable);
    }

    let recovery = lock_recovery(&mut transaction, command.recovery_id.as_uuid()).await?;
    let current_equipment = recovery_equipment_identity(&mut transaction, &recovery).await?;
    ensure_preloaded_equipment(preloaded, current_equipment.as_ref())?;
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
    // 规范锁序：恢复动作 → 投递 → 履约。原付款与交付投递在履约行之前锁定，与 Admin 投递命令
    // （先锁投递、再改写履约）在投递行上串行，不形成锁环（不变量 2）。
    lock_obligation_deliveries(&mut transaction, &obligations).await?;
    let selected_action = validate_actions(&recovery, &actions, command.action)?;
    validate_custody_evidence(&recovery, selected_action)?;
    validate_obligations(&mut transaction, &recovery, &obligations, command.action).await?;

    let now = database_now(&mut transaction).await?;
    if command.action == RecoveryConfirmationAction::EquipmentRecovered {
        activate_buyer_refund(&mut transaction, &recovery, now).await?;
        // 已付款交付被收回：卖方货款与原交付不再可执行，与退款激活同事务收口；原交付与本轮
        // Take 的链路不精确相符时整个确认回滚（不变量 2、4、6、8）。
        close_recovered_dependents(&mut transaction, &recovery, selected_action, &command, now)
            .await?;
    }
    // 成交撤销与原付款及依赖履约的收口同事务提交：确认之后不能再有旧付款被领取、准备或满足
    // （不变量 2、4、6、8）。
    close_voided_fulfillments(&mut transaction, &recovery, &command, now).await?;

    let relist =
        relist_recovery(&mut transaction, &recovery, &lot, preview_event_id, command.action, now)
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
    // resolution 由 NULL 首次落定即表示成交撤销；补偿池冲正必须与之同事务提交，
    // 不能留下“已撤销但池中仍计入附加费/补偿”的中间状态。
    crate::database::settlement::compensation_reversal::reverse_voided_sale(
        &mut transaction,
        recovery.id,
        now,
    )
    .await
    .map_err(map_reversal_error)?;

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

    if let Err(error) = insert_recovery_command(&mut transaction, &command, &result, now).await {
        if matches!(error, RecoveryCommandError::CommandConflict) {
            // PostgreSQL 在唯一冲突后会将当前事务置为 failed；必须先显式回滚，
            // 才能用新事务观察竞争者已经提交的命令账本结果。
            transaction.rollback().await.map_err(map_sql_error)?;
            return replay_after_command_conflict(pool, &command).await;
        }
        return Err(error);
    }
    insert_recovery_audit(&mut transaction, &command, &recovery, selected_action, &result).await?;

    transaction.commit().await.map_err(map_sql_error)?;
    Ok(result)
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

async fn replay_after_command_conflict(
    pool: &crate::database::TrackedPool<Postgres>,
    command: &RecoveryConfirmationCommand,
) -> Result<RecoveryConfirmationResult, RecoveryCommandError> {
    let mut replay_transaction = pool.begin().await.map_err(map_sql_error)?;
    let replay = load_replayed_command(&mut replay_transaction, command).await;
    match replay {
        Ok(Some(result)) => {
            replay_transaction.commit().await.map_err(map_sql_error)?;
            Ok(result)
        }
        Ok(None) => {
            // 主键冲突却找不到已提交 ledger 是持久化事实不完整，失败关闭。
            replay_transaction.rollback().await.map_err(map_sql_error)?;
            Err(RecoveryCommandError::InvalidStoredResult)
        }
        Err(error) => {
            replay_transaction.rollback().await.map_err(map_sql_error)?;
            Err(error)
        }
    }
}
