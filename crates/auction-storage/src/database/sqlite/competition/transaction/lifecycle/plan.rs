//! SQLite 生命周期计划校验与状态写入。

use super::super::mapping::*;
use super::super::*;

pub(super) async fn apply_lifecycle_plan_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    plan: AuctionLifecyclePlan,
) -> Result<(), CompetitionTransactionError> {
    if plan.auction.id != command.auction_id || plan.effect.action() != command.action {
        return Err(invalid_state("Lite 生命周期命令与动作计划不一致"));
    }
    let AuctionLifecyclePlan { auction, effect } = plan;
    match effect {
        AuctionLifecycleEffect::Preview { manual, successor } => {
            apply_preview_plan_sqlite(transaction, command, &auction, manual, &successor).await
        }
        AuctionLifecycleEffect::Open { manual_override } => {
            apply_open_plan_sqlite(transaction, command, &auction, manual_override).await
        }
        AuctionLifecycleEffect::Cancel => apply_cancel_plan_sqlite(transaction, &auction).await,
        AuctionLifecycleEffect::FinalPhase { proxy_plans } => {
            apply_final_plan_sqlite(transaction, command, &auction, proxy_plans).await
        }
    }
}

async fn apply_preview_plan_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    manual: bool,
    successor: &AuctionPreviewSuccessorPlan,
) -> Result<(), CompetitionTransactionError> {
    let world_id: String = {
        let connection = transaction.connection_mut()?;
        let row = sqlx::query(
            "SELECT auction.world_id, world.enabled, auction.predecessor_auction_id
             FROM auctions AS auction
             INNER JOIN worlds AS world ON world.id = auction.world_id
             WHERE auction.id = ?",
        )
        .bind(uuid_blob(auction.id.as_uuid()))
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_sql_error)?
        .ok_or(CompetitionTransactionError::NotFound)?;
        if row.try_get::<i64, _>("enabled").map_err(map_sql_error)? == 0 {
            return Err(CompetitionTransactionError::InvalidState);
        }
        if let Some(predecessor) =
            row.try_get::<Option<Vec<u8>>, _>("predecessor_auction_id").map_err(map_sql_error)?
        {
            let predecessor = sqlx::query("SELECT world_id, status FROM auctions WHERE id = ?")
                .bind(predecessor)
                .fetch_optional(&mut *connection)
                .await
                .map_err(map_sql_error)?
                .ok_or_else(|| invalid_state("Lite 预览拍卖的前驱记录不存在"))?;
            let predecessor_world: String =
                predecessor.try_get("world_id").map_err(map_sql_error)?;
            let predecessor_status: String =
                predecessor.try_get("status").map_err(map_sql_error)?;
            let current_world: String = row.try_get("world_id").map_err(map_sql_error)?;
            if predecessor_world != current_world
                || !matches!(predecessor_status.as_str(), "settled" | "cancelled")
            {
                return Err(CompetitionTransactionError::InvalidState);
            }
        }
        row.try_get("world_id").map_err(map_sql_error)?
    };
    let now = sqlite_timestamp(auction.updated_at);
    let preview_revision = {
        let connection = transaction.connection_mut()?;
        sqlx::query_scalar::<_, i64>(
            "UPDATE auctions
             SET status = 'preview', preview_opened_at = COALESCE(preview_opened_at, ?),
                 preview_manual_open = CASE WHEN preview_manual_open = 1 OR ? THEN 1 ELSE 0 END,
                 preview_revision = preview_revision + 1, state_version = ?, updated_at = ?
             WHERE id = ? AND status = 'draft' AND state_version = ?
               AND (? OR preview_scheduled_at <= ?)
             RETURNING preview_revision",
        )
        .bind(&now)
        .bind(manual)
        .bind(i64_version(auction.state_version, "Lite 预览状态版本")?)
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .bind(previous_lifecycle_version_sqlite(auction)?)
        .bind(manual)
        .bind(&now)
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_sql_error)?
    };
    if preview_revision.is_none_or(|value| value <= 0) {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let materialized = {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "INSERT INTO auction_preview_items
                (auction_id, equipment_id, display_order, start_price, source_revision, collected_at)
             SELECT lot.auction_id, lot.equipment_id, lot.lot_number, lot.start_price,
                    auction.preview_revision, equipment.collected_at
             FROM lots AS lot
             INNER JOIN auctions AS auction ON auction.id = lot.auction_id
             INNER JOIN equipment AS equipment ON equipment.id = lot.equipment_id
             WHERE lot.auction_id = ? AND lot.status = 'pending' AND auction.status = 'preview'
             ORDER BY lot.lot_number",
        )
        .bind(uuid_blob(auction.id.as_uuid()))
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?
        .rows_affected()
    };
    let pending_count: i64 = {
        let connection = transaction.connection_mut()?;
        sqlx::query_scalar("SELECT COUNT(*) FROM lots WHERE auction_id = ? AND status = 'pending'")
            .bind(uuid_blob(auction.id.as_uuid()))
            .fetch_one(&mut *connection)
            .await
            .map_err(map_sql_error)?
    };
    if i64::try_from(materialized).ok() != Some(pending_count) {
        return Err(invalid_state("Lite 预览物化条目数与 pending lot 不一致"));
    }

    let successor_version = {
        let connection = transaction.connection_mut()?;
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO auctions
                (id, world_id, sequence, title, status, default_start_price,
                 soft_close_window_seconds, soft_close_extension_seconds,
                 cancel_cutoff_seconds, proxy_slow_interval_seconds, policy_snapshot,
                 preview_scheduled_at, scheduled_open_at, predecessor_auction_id,
                 state_version, created_at, updated_at)
             SELECT ?, current.world_id, ?, ?, 'draft', current.default_start_price,
                    current.soft_close_window_seconds, current.soft_close_extension_seconds,
                    current.cancel_cutoff_seconds, current.proxy_slow_interval_seconds,
                    json_set(current.policy_snapshot, '$.scheduled_end_at', ?),
                    ?, ?, current.id, 1, ?, ?
             FROM auctions AS current
             WHERE current.id = ? AND current.status = 'preview'
             RETURNING state_version",
        )
        .bind(uuid_blob(successor.auction_id.as_uuid()))
        .bind(successor.sequence)
        .bind(&successor.title)
        .bind(successor.scheduled_end_at.to_rfc3339())
        .bind(sqlite_timestamp(successor.preview_due_at))
        .bind(sqlite_timestamp(successor.open_due_at))
        .bind(&now)
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_sql_error)?
    };
    if successor_version != Some(1) {
        return Err(invalid_state("Lite 下一期草稿不满足初始状态不变量"));
    }

    transaction
        .insert_command_inner(StoredCommand {
            command_id: successor.create_command_id,
            command_kind: "scheduled_auction_create".to_owned(),
            aggregate_id: successor.auction_id.as_uuid(),
            request_hash: successor.create_request_hash,
            result_code: "accepted".to_owned(),
            result_payload: serde_json::to_vec(&json!({
                "command_id": successor.create_command_id.as_uuid(),
                "auction_id": successor.auction_id.as_uuid(),
                "status": "draft",
                "state_version": 1,
            }))
            .map_err(|_| invalid_record())?,
        })
        .await?;
    {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "INSERT INTO scheduled_rounds
                (id, world_id, local_week, auction_id, status, preview_due_at,
                 open_due_at, scheduled_end_at, create_command_id,
                 state_version, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'active', ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(uuid_blob(successor.round_id))
        .bind(&world_id)
        .bind(successor.local_week.format("%Y-%m-%d").to_string())
        .bind(uuid_blob(successor.auction_id.as_uuid()))
        .bind(sqlite_timestamp(successor.preview_due_at))
        .bind(sqlite_timestamp(successor.open_due_at))
        .bind(sqlite_timestamp(successor.scheduled_end_at))
        .bind(uuid_blob(successor.create_command_id.as_uuid()))
        .bind(&now)
        .bind(&now)
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?;
        if manual {
            sqlx::query(
                "UPDATE scheduled_rounds
                 SET preview_command_id = ?, preview_completed_at = ?,
                     state_version = state_version + 1, updated_at = ?
                 WHERE auction_id = ? AND status = 'active' AND preview_completed_at IS NULL",
            )
            .bind(uuid_blob(command.command_id.as_uuid()))
            .bind(&now)
            .bind(&now)
            .bind(uuid_blob(auction.id.as_uuid()))
            .execute(&mut *connection)
            .await
            .map_err(map_sql_error)?;
        }
    }
    Ok(())
}

async fn apply_open_plan_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    manual_override: bool,
) -> Result<(), CompetitionTransactionError> {
    let scheduled_end_at =
        auction.scheduled_end_at.ok_or_else(|| invalid_state("Lite 开拍计划缺少名义结束时间"))?;
    if auction.effective_end_at != Some(scheduled_end_at) {
        return Err(invalid_state("Lite 开拍计划的名义与有效结束时间不一致"));
    }
    let lot_rows = {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "SELECT status, submission_id FROM lots WHERE auction_id = ? ORDER BY lot_number",
        )
        .bind(uuid_blob(auction.id.as_uuid()))
        .fetch_all(&mut *connection)
        .await
        .map_err(map_sql_error)?
    };
    if lot_rows.is_empty()
        || lot_rows.iter().any(|row| {
            row.try_get::<String, _>("status").ok().as_deref() != Some("pending")
                || row.try_get::<Option<Vec<u8>>, _>("submission_id").ok().flatten().is_none()
        })
    {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let now = sqlite_timestamp(auction.updated_at);
    let activated = {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "UPDATE lots
             SET status = 'live', current_price = start_price, effective_end_at = ?,
                 slow_due_at = NULL, slow_authorization_id = NULL, slow_generation = 0,
                 last_proxy_sequence = 0, state_version = state_version + 1, updated_at = ?
             WHERE auction_id = ? AND status = 'pending'",
        )
        .bind(sqlite_timestamp(scheduled_end_at))
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?
        .rows_affected()
    };
    if usize::try_from(activated).ok() != Some(lot_rows.len()) {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let updated = {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "UPDATE auctions
             SET status = 'live', preview_opened_at = COALESCE(preview_opened_at, ?),
                 preview_scheduled_at = CASE
                     WHEN ? AND preview_scheduled_at > ? THEN ? ELSE preview_scheduled_at END,
                 scheduled_open_at = CASE WHEN ? THEN ? ELSE scheduled_open_at END,
                 opened_at = ?, scheduled_end_at = ?, effective_end_at = ?,
                 state_version = ?, updated_at = ?
             WHERE id = ? AND status = 'preview' AND state_version = ?
               AND (preview_scheduled_at <= ? OR ?)
               AND (scheduled_open_at <= ? OR ?)",
        )
        .bind(&now)
        .bind(manual_override)
        .bind(&now)
        .bind(&now)
        .bind(manual_override)
        .bind(&now)
        .bind(&now)
        .bind(sqlite_timestamp(scheduled_end_at))
        .bind(sqlite_timestamp(scheduled_end_at))
        .bind(i64_version(auction.state_version, "Lite 开拍状态版本")?)
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .bind(previous_lifecycle_version_sqlite(auction)?)
        .bind(&now)
        .bind(manual_override)
        .bind(&now)
        .bind(manual_override)
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?
    };
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }
    if command.actor.actor_type == auction_application::AuctionLifecycleActorType::Admin {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "UPDATE scheduled_rounds
             SET open_command_id = ?, open_completed_at = ?,
                 state_version = state_version + 1, updated_at = ?
             WHERE auction_id = ? AND status = 'active'
               AND preview_completed_at IS NOT NULL AND open_completed_at IS NULL",
        )
        .bind(uuid_blob(command.command_id.as_uuid()))
        .bind(&now)
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

async fn apply_cancel_plan_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    auction: &Auction,
) -> Result<(), CompetitionTransactionError> {
    let previous_status: String = {
        let connection = transaction.connection_mut()?;
        sqlx::query_scalar("SELECT status FROM auctions WHERE id = ?")
            .bind(uuid_blob(auction.id.as_uuid()))
            .fetch_optional(&mut *connection)
            .await
            .map_err(map_sql_error)?
            .ok_or(CompetitionTransactionError::NotFound)?
    };
    if !matches!(previous_status.as_str(), "draft" | "preview") {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let now = sqlite_timestamp(auction.updated_at);
    let connection = transaction.connection_mut()?;
    sqlx::query(
        "UPDATE lots
         SET status = 'withdrawn', slow_authorization_id = NULL, slow_due_at = NULL,
             state_version = state_version + 1, updated_at = ?
         WHERE auction_id = ? AND status = 'pending'",
    )
    .bind(&now)
    .bind(uuid_blob(auction.id.as_uuid()))
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "UPDATE scheduled_rounds
         SET status = 'missed', auction_id = NULL, lease_owner = NULL, lease_until = NULL,
             state_version = state_version + 1, updated_at = ?
         WHERE auction_id = ? AND status = 'active' AND open_completed_at IS NULL",
    )
    .bind(&now)
    .bind(uuid_blob(auction.id.as_uuid()))
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    let updated = sqlx::query(
        "UPDATE auctions
         SET status = 'cancelled', state_version = ?, updated_at = ?,
             cancelled_at = ?, cancelled_reason = ?, cancelled_by = ?
         WHERE id = ? AND status = ? AND state_version = ?",
    )
    .bind(i64_version(auction.state_version, "Lite 取消状态版本")?)
    .bind(&now)
    .bind(auction.cancelled_at.map(sqlite_timestamp))
    .bind(&auction.cancelled_reason)
    .bind(&auction.cancelled_by)
    .bind(uuid_blob(auction.id.as_uuid()))
    .bind(previous_status)
    .bind(previous_lifecycle_version_sqlite(auction)?)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }
    Ok(())
}

async fn apply_final_plan_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    proxy_plans: Vec<auction_application::AuctionFinalProxyPlan>,
) -> Result<(), CompetitionTransactionError> {
    let now = sqlite_timestamp(auction.updated_at);
    let updated = {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "UPDATE auctions
             SET status = 'final', final_phase_at = ?, state_version = ?, updated_at = ?
             WHERE id = ? AND status = 'live' AND state_version = ?",
        )
        .bind(&now)
        .bind(i64_version(auction.state_version, "Lite 最终阶段状态版本")?)
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .bind(previous_lifecycle_version_sqlite(auction)?)
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?
    };
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }
    for proxy_plan in proxy_plans {
        if proxy_plan.command.aggregate_id != proxy_plan.competition.final_lot.lot_id.as_uuid() {
            return Err(invalid_state("Lite 最终阶段代理子命令与 lot 计划不一致"));
        }
        transaction.insert_command_inner(proxy_plan.command).await?;
        transaction.apply_plan_inner(proxy_plan.competition, command.request_id.clone()).await?;
    }
    if command.actor.actor_type == auction_application::AuctionLifecycleActorType::Admin {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "UPDATE scheduled_rounds
             SET final_command_id = ?, final_completed_at = ?,
                 lease_owner = NULL, lease_until = NULL,
                 state_version = state_version + 1, updated_at = ?
             WHERE auction_id = ? AND status = 'active'
               AND open_completed_at IS NOT NULL AND final_completed_at IS NULL",
        )
        .bind(uuid_blob(command.command_id.as_uuid()))
        .bind(&now)
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

fn previous_lifecycle_version_sqlite(
    auction: &Auction,
) -> Result<i64, CompetitionTransactionError> {
    auction
        .state_version
        .checked_sub(1)
        .and_then(|version| i64::try_from(version).ok())
        .ok_or_else(|| invalid_state("Lite 拍卖状态版本无法回退校验"))
}
