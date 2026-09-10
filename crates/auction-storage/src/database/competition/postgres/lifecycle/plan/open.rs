//! PostgreSQL 开拍阶段计划写入。

use super::super::record::previous_lifecycle_version;
use super::super::*;

pub(super) async fn apply_open_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    manual_override: bool,
    successor: &AuctionPreviewSuccessorPlan,
) -> Result<(), CompetitionTransactionError> {
    let scheduled_end_at =
        auction.scheduled_end_at.ok_or_else(|| invalid_state("开拍计划缺少名义结束时间"))?;
    if auction.effective_end_at != Some(scheduled_end_at) {
        return Err(invalid_state("开拍计划的名义与有效结束时间不一致"));
    }
    // 生命周期上下文已经按 world → round → auction 加锁；这里继续锁住同一录入目标，
    // 让并发装备录入要么先进入本期 Preview，要么在切换完成后进入下一期 Draft。
    let intake_target = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT world_id, target_status, state_version
             FROM public.auction_intake_targets
             WHERE auction_id = $1
             FOR UPDATE",
        )
        .bind(auction.id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
        .ok_or(CompetitionTransactionError::InvalidState)?
    };
    let world_id: String = intake_target.try_get("world_id").map_err(map_sql_error)?;
    let target_status: String = intake_target.try_get("target_status").map_err(map_sql_error)?;
    let target_version: i64 = intake_target.try_get("state_version").map_err(map_sql_error)?;
    if target_status != "preview" || target_version <= 0 {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let lot_rows = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT id, status, submission_id
             FROM public.lots
             WHERE auction_id = $1
             ORDER BY lot_number
             FOR UPDATE",
        )
        .bind(auction.id.as_uuid())
        .fetch_all(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    // 已撤回的拍品是保留的历史事实，不再参与开拍；仍锁住并检查全部记录，避免把其他
    // 非法状态当作可忽略历史。只有具备收件来源的 pending 拍品才能进入本次激活集合。
    let mut pending_count = 0_usize;
    for row in &lot_rows {
        let status: String = row.try_get("status").map_err(map_sql_error)?;
        match status.as_str() {
            "pending" => {
                if row.try_get::<Option<Uuid>, _>("submission_id").map_err(map_sql_error)?.is_none()
                {
                    return Err(CompetitionTransactionError::InvalidState);
                }
                pending_count += 1;
            }
            "withdrawn" => {}
            _ => return Err(CompetitionTransactionError::InvalidState),
        }
    }
    if pending_count == 0 {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let activated = {
        let transaction = transaction.transaction_mut()?;
        // 新存储以 NULL 表示尚无领先者；旧版起拍价哨兵只在竞价与公开读取防腐层归一化，
        // 避免把持久化占位误当成已经存在的竞价事实。
        sqlx::query(
            "UPDATE public.lots
             SET status = 'live', current_price = NULL, effective_end_at = $1,
                 slow_due_at = NULL, slow_authorization_id = NULL, slow_generation = 0,
                 last_proxy_sequence = 0, state_version = state_version + 1, updated_at = $2
             WHERE auction_id = $3 AND status = 'pending'",
        )
        .bind(scheduled_end_at)
        .bind(auction.updated_at)
        .bind(auction.id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?
        .rows_affected()
    };
    if usize::try_from(activated).ok() != Some(pending_count) {
        return Err(CompetitionTransactionError::InvalidState);
    }

    let updated = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.auctions
             SET status = 'live',
                 preview_opened_at = COALESCE(preview_opened_at, $1),
                 preview_scheduled_at = CASE
                     WHEN $2 AND preview_scheduled_at > $1 THEN $1 ELSE preview_scheduled_at END,
                 scheduled_open_at = CASE WHEN $2 THEN $1 ELSE scheduled_open_at END,
                 opened_at = $1, scheduled_end_at = $3, effective_end_at = $3,
                 state_version = $4, updated_at = $1
             WHERE id = $5 AND status = 'preview' AND state_version = $6
               AND (preview_scheduled_at <= $1 OR $2)
               AND (scheduled_open_at <= $1 OR $2)",
        )
        .bind(auction.updated_at)
        .bind(manual_override)
        .bind(scheduled_end_at)
        .bind(i64_version(auction.state_version, "开拍状态版本")?)
        .bind(auction.id.as_uuid())
        .bind(previous_lifecycle_version(auction)?)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if updated.rows_affected() != 1 {
        return Err(CompetitionTransactionError::InvalidState);
    }

    super::successor::insert_successor_postgres(transaction, auction, "live", &world_id, successor)
        .await?;
    {
        let transaction = transaction.transaction_mut()?;
        let moved = sqlx::query(
            "UPDATE public.auction_intake_targets
             SET auction_id = $1, target_status = 'draft',
                 state_version = state_version + 1, updated_at = $2
             WHERE world_id = $3 AND auction_id = $4 AND target_status = 'preview'
               AND state_version = $5",
        )
        .bind(successor.auction_id.as_uuid())
        .bind(auction.updated_at)
        .bind(&world_id)
        .bind(auction.id.as_uuid())
        .bind(target_version)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
        if moved.rows_affected() != 1 {
            return Err(CompetitionTransactionError::InvalidState);
        }
    }

    if command.actor.actor_type == auction_application::AuctionLifecycleActorType::Admin {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.scheduled_rounds
             SET open_command_id = $1, open_completed_at = $2,
                 state_version = state_version + 1, updated_at = $2
             WHERE auction_id = $3 AND status = 'active'
               AND preview_completed_at IS NOT NULL AND open_completed_at IS NULL",
        )
        .bind(command.command_id.as_uuid())
        .bind(auction.updated_at)
        .bind(auction.id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}
