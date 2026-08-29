//! PostgreSQL 开拍阶段计划写入。

use super::super::record::previous_lifecycle_version;
use super::super::*;

pub(super) async fn apply_open_plan_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    manual_override: bool,
) -> Result<(), CompetitionTransactionError> {
    let scheduled_end_at =
        auction.scheduled_end_at.ok_or_else(|| invalid_state("开拍计划缺少名义结束时间"))?;
    if auction.effective_end_at != Some(scheduled_end_at) {
        return Err(invalid_state("开拍计划的名义与有效结束时间不一致"));
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
    if lot_rows.is_empty()
        || lot_rows.iter().any(|row| {
            row.try_get::<String, _>("status").ok().as_deref() != Some("pending")
                || row.try_get::<Option<Uuid>, _>("submission_id").ok().flatten().is_none()
        })
    {
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
    if usize::try_from(activated).ok() != Some(lot_rows.len()) {
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
