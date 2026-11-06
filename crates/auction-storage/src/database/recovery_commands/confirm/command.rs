//! Admin recovery 确认命令及其 Serializable 重试。

#[cfg(feature = "postgres")]
use {
    crate::database::recovery_commands::{
        activate_buyer_refund, database_now, insert_recovery_audit, insert_recovery_command,
        is_conflict, load_recovery_scope, load_replayed_command, lock_actions, lock_auction,
        lock_obligations, lock_original_lot, lock_recovery, lock_world, map_reversal_error,
        map_sql_error, relist_recovery, update_recovery, validate_actions,
        validate_custody_evidence, validate_obligations, validate_recovery_row,
    },
    auction_application::recovery::{
        RecoveryCommandError, RecoveryConfirmationAction, RecoveryConfirmationCommand,
        RecoveryConfirmationResult,
    },
    sqlx::Postgres,
    uuid::Uuid,
};

#[cfg(feature = "postgres")]
pub(in crate::database::recovery_commands) async fn confirm_postgres(
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

#[cfg(feature = "postgres")]
pub(in crate::database::recovery_commands) async fn confirm_postgres_once(
    pool: &crate::database::TrackedPool<Postgres>,
    command: RecoveryConfirmationCommand,
    preview_event_id: Uuid,
) -> Result<RecoveryConfirmationResult, RecoveryCommandError> {
    let mut transaction =
        pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await.map_err(map_sql_error)?;

    if let Some(result) = load_replayed_command(&mut transaction, &command).await? {
        transaction.commit().await.map_err(map_sql_error)?;
        return Ok(result);
    }

    let scope = load_recovery_scope(&mut transaction, command.recovery_id.as_uuid()).await?;
    // 确认必然落定 resolution 并冲正补偿池；先锁池再锁世界，与结算的锁顺序一致。
    crate::database::compensation_reversal::lock_pool(&mut transaction)
        .await
        .map_err(map_reversal_error)?;
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

    let relist = relist_recovery(
        &mut transaction,
        &recovery,
        &lot,
        command.command_id,
        preview_event_id,
        command.action,
        now,
    )
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
    crate::database::compensation_reversal::reverse_voided_sale(&mut transaction, recovery.id, now)
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

#[cfg(feature = "postgres")]
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
