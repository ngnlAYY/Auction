//! SQLite preview 生命周期动作。

use super::super::super::mapping::*;
use super::super::super::*;

pub(super) async fn apply_preview_plan_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    auction: &Auction,
    manual: bool,
) -> Result<(), CompetitionTransactionError> {
    {
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
    }
    let now = sqlite_timestamp(auction.updated_at);

    // SQLite 串行写事务同样要在状态写入前拒绝空目录，不能留下无法开放的 preview 拍卖。
    let pending_count: i64 = {
        let connection = transaction.connection_mut()?;
        sqlx::query_scalar("SELECT COUNT(*) FROM lots WHERE auction_id = ? AND status = 'pending'")
            .bind(uuid_blob(auction.id.as_uuid()))
            .fetch_one(&mut *connection)
            .await
            .map_err(map_sql_error)?
    };
    if pending_count <= 0 {
        return Err(invalid_state("Lite 预览至少需要一个 pending lot"));
    }

    let preview_revision = {
        let connection = transaction.connection_mut()?;
        sqlx::query_scalar::<_, i64>(
            "UPDATE auctions
             SET status = 'preview', preview_opened_at = COALESCE(preview_opened_at, ?),
                 preview_manual_open = CASE WHEN preview_manual_open = 1 OR ? THEN 1 ELSE 0 END,
                 state_version = ?, updated_at = ?
             WHERE id = ? AND status = 'draft' AND state_version = ?
               AND (? OR preview_scheduled_at <= ?)
             RETURNING preview_revision",
        )
        .bind(&now)
        .bind(manual)
        .bind(i64_version(auction.state_version, "Lite 预览状态版本")?)
        .bind(&now)
        .bind(uuid_blob(auction.id.as_uuid()))
        .bind(super::previous_lifecycle_version_sqlite(auction)?)
        .bind(manual)
        .bind(&now)
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_sql_error)?
    };
    if preview_revision.is_none_or(|value| value < 0) {
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
    if i64::try_from(materialized).ok() != Some(pending_count) {
        return Err(invalid_state("Lite 预览物化条目数与 pending lot 不一致"));
    }

    if manual {
        let connection = transaction.connection_mut()?;
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
    Ok(())
}
