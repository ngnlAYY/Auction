//! SQLite open 生命周期动作。

use super::super::super::mapping::*;
use super::super::super::*;

pub(super) async fn apply_open_plan_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    manual_override: bool,
    successor: &AuctionPreviewSuccessorPlan,
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
    // 单写者事务中保留 withdrawn 历史，只激活有投稿来源的 pending 拍品；其他状态
    // 仍然失败关闭，不能通过过滤查询掩盖损坏的生命周期记录。
    let mut pending_count = 0_usize;
    for row in &lot_rows {
        let status: String = row.try_get("status").map_err(map_sql_error)?;
        match status.as_str() {
            "pending" => {
                if row
                    .try_get::<Option<Vec<u8>>, _>("submission_id")
                    .map_err(map_sql_error)?
                    .is_none()
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
    let now = sqlite_timestamp(auction.updated_at);
    let activated = {
        let connection = transaction.connection_mut()?;
        // Lite 与 PostgreSQL 共用 NULL 无领先者语义；旧版起拍价哨兵只在边界防腐层
        // 归一化，不能在领域层伪造成已经存在的竞价事实。
        sqlx::query(
            "UPDATE lots
             SET status = 'live', current_price = NULL, effective_end_at = ?,
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
    if usize::try_from(activated).ok() != Some(pending_count) {
        return Err(CompetitionTransactionError::InvalidState);
    }
    let updated = {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "UPDATE auctions
             SET status = 'live', preview_opened_at = COALESCE(preview_opened_at, ?),
                 preview_scheduled_at = CASE
                     WHEN ? AND julianday(preview_scheduled_at) > julianday(?)
                     THEN ? ELSE preview_scheduled_at END,
                 scheduled_open_at = CASE WHEN ? THEN ? ELSE scheduled_open_at END,
                 opened_at = ?, scheduled_end_at = ?, effective_end_at = ?,
                 state_version = ?, updated_at = ?
             WHERE id = ? AND status = 'preview' AND state_version = ?
               AND (julianday(preview_scheduled_at) <= julianday(?) OR ?)
               AND (julianday(scheduled_open_at) <= julianday(?) OR ?)",
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
        .bind(super::previous_lifecycle_version_sqlite(auction)?)
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
    super::successor::insert_successor_sqlite(transaction, auction, "live", successor).await?;
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
