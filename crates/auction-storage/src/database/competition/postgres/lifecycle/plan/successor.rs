//! PostgreSQL 下一期草稿和持久化周轮次写入。

use super::super::*;

/// 在当前拍卖已经停止接收装备后建立下一期草稿。
///
/// 调用方必须在同一事务中先把当前 Preview 推进为 Live 或 Cancelled，再调用本函数；这样
/// `auctions_one_intake_world_idx` 在任意 SQL 语句结束时都不会同时看到 Preview 与 Draft。
pub(super) async fn insert_successor_postgres(
    transaction: &mut PostgresCompetitionTransaction,
    auction: &Auction,
    current_status: &str,
    world_id: &str,
    successor: &AuctionPreviewSuccessorPlan,
) -> Result<(), CompetitionTransactionError> {
    if !matches!(current_status, "live" | "cancelled") {
        return Err(invalid_state("下一期草稿的前驱状态无效"));
    }
    let successor_row = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            r#"INSERT INTO public.auctions
                (id, world_id, sequence, title, status, preview_scheduled_at,
                 scheduled_open_at, default_start_price, soft_close_window_seconds,
                 soft_close_extension_seconds, cancel_cutoff_seconds,
                 proxy_slow_interval_seconds, policy_snapshot, predecessor_auction_id,
                 created_at, updated_at)
             SELECT $1, current.world_id, $2, $3, 'draft', $4, $5,
                    current.default_start_price, current.soft_close_window_seconds,
                    current.soft_close_extension_seconds, current.cancel_cutoff_seconds,
                    current.proxy_slow_interval_seconds,
                    jsonb_set(current.policy_snapshot, '{scheduled_end_at}',
                              to_jsonb($6::timestamptz), true),
                    current.id, $7, $7
             FROM public.auctions AS current
             WHERE current.id = $8 AND current.status = $9
             RETURNING world_id, state_version"#,
        )
        .bind(successor.auction_id.as_uuid())
        .bind(successor.sequence)
        .bind(&successor.title)
        .bind(successor.preview_due_at)
        .bind(successor.open_due_at)
        .bind(successor.scheduled_end_at)
        .bind(auction.updated_at)
        .bind(auction.id.as_uuid())
        .bind(current_status)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(successor_row) = successor_row else {
        return Err(CompetitionTransactionError::InvalidState);
    };
    let successor_world: String = successor_row.try_get("world_id").map_err(map_sql_error)?;
    let successor_version: i64 = successor_row.try_get("state_version").map_err(map_sql_error)?;
    if successor_world != world_id || successor_version != 1 {
        return Err(invalid_state("下一期草稿不满足初始状态不变量"));
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

    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        "INSERT INTO public.scheduled_rounds
            (id, world_id, local_week, auction_id, status, preview_due_at,
             open_due_at, scheduled_end_at, create_command_id, created_at, updated_at, schedule_snapshot)
         VALUES ($1, $2, $3, $4, 'active', $5, $6, $7, $8, $9, $9, $10)",
    )
    .bind(successor.round_id)
    .bind(world_id)
    .bind(successor.local_week)
    .bind(successor.auction_id.as_uuid())
    .bind(successor.preview_due_at)
    .bind(successor.open_due_at)
    .bind(successor.scheduled_end_at)
    .bind(successor.create_command_id.as_uuid())
    .bind(auction.updated_at)
    .bind(successor.schedule_source.as_ref().map(serde_json::to_value).transpose().map_err(|_| invalid_record())?)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}
