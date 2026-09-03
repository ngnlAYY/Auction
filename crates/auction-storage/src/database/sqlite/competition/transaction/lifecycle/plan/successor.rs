//! SQLite 下一期草稿和持久化周轮次写入。

use super::super::super::mapping::*;
use super::super::super::*;

/// 在当前拍卖已经停止接收装备后建立下一期 Lite 草稿。
pub(super) async fn insert_successor_sqlite(
    transaction: &mut SqliteCompetitionTransaction,
    auction: &Auction,
    current_status: &str,
    successor: &AuctionPreviewSuccessorPlan,
) -> Result<(), CompetitionTransactionError> {
    if !matches!(current_status, "live" | "cancelled") {
        return Err(invalid_state("Lite 下一期草稿的前驱状态无效"));
    }
    let now = sqlite_timestamp(auction.updated_at);
    let (world_id, successor_version) = {
        let connection = transaction.connection_mut()?;
        let row = sqlx::query(
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
             WHERE current.id = ? AND current.status = ?
             RETURNING world_id, state_version",
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
        .bind(current_status)
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_sql_error)?
        .ok_or(CompetitionTransactionError::InvalidState)?;
        (
            row.try_get::<String, _>("world_id").map_err(map_sql_error)?,
            row.try_get::<i64, _>("state_version").map_err(map_sql_error)?,
        )
    };
    if successor_version != 1 {
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
                "state_version": successor_version,
            }))
            .map_err(|_| invalid_record())?,
        })
        .await?;

    let connection = transaction.connection_mut()?;
    sqlx::query(
        "INSERT INTO scheduled_rounds
            (id, world_id, local_week, auction_id, status, preview_due_at,
             open_due_at, scheduled_end_at, create_command_id,
             state_version, created_at, updated_at)
         VALUES (?, ?, ?, ?, 'active', ?, ?, ?, ?, 1, ?, ?)",
    )
    .bind(uuid_blob(successor.round_id))
    .bind(world_id)
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
    Ok(())
}
