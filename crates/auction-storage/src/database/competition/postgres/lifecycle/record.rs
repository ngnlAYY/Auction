//! PostgreSQL 拍卖生命周期审计与实时事件记录。

use super::*;

pub(super) fn previous_lifecycle_version(
    auction: &Auction,
) -> Result<i64, CompetitionTransactionError> {
    auction
        .state_version
        .checked_sub(1)
        .and_then(|version| i64::try_from(version).ok())
        .ok_or_else(|| invalid_state("拍卖状态版本无法回退校验"))
}

#[allow(clippy::manual_async_fn)]
pub(super) async fn append_lifecycle_record_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    result: &AuctionLifecycleResult,
) -> Result<(), CompetitionTransactionError> {
    let metadata = json!({
        "command_id": command.command_id.as_uuid(),
        "action": command.action.command_kind(),
        "status": auction_status_name(result.status),
        "state_version": result.state_version,
    });
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        r#"INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
         VALUES ($1, $2, $3, $4, 'auction', $5, 'success', $6, $7, $8)"#,
    )
    .bind(Uuid::new_v4())
    .bind(command.actor.actor_type.database_name())
    .bind(&command.actor.actor_id)
    .bind(command.action.command_kind())
    .bind(result.auction_id.as_uuid().to_string())
    .bind(command.request_id.as_deref())
    .bind(command.actor.actor_type.database_name())
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;

    let (event_kind, payload) = lifecycle_event_payload(transaction, command, result).await?;
    let stream_key = format!("auction:{}", result.auction_id.as_uuid());
    sqlx::query(
        "INSERT INTO public.realtime_streams (stream_key)
         VALUES ($1) ON CONFLICT (stream_key) DO NOTHING",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let sequence: i64 = sqlx::query_scalar(
        "SELECT next_sequence FROM public.realtime_streams WHERE stream_key = $1 FOR UPDATE",
    )
    .bind(&stream_key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "UPDATE public.realtime_streams
         SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
         WHERE stream_key = $1",
    )
    .bind(&stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        r#"INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id)
         VALUES ($1, $2, $3, $4, 1, $5, $6, $7)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&stream_key)
    .bind(sequence)
    .bind(event_kind)
    .bind(payload)
    .bind(i64_version(result.state_version, "生命周期事件状态版本")?)
    .bind(command.command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

async fn lifecycle_event_payload(
    transaction: &mut PgTransaction,
    command: &AuctionLifecycleCommand,
    result: &AuctionLifecycleResult,
) -> Result<(&'static str, Value), CompetitionTransactionError> {
    let row = sqlx::query(
        r#"SELECT a.world_id,
                  a.preview_revision,
                  a.preview_manual_open,
                  (SELECT COUNT(*)::bigint
                   FROM public.auction_preview_items AS item
                   WHERE item.auction_id = a.id) AS item_count
           FROM public.auctions AS a
           WHERE a.id = $1"#,
    )
    .bind(result.auction_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let world_id: String = row.try_get("world_id").map_err(map_sql_error)?;
    let preview_revision: i64 = row.try_get("preview_revision").map_err(map_sql_error)?;
    let manual_open: bool = row.try_get("preview_manual_open").map_err(map_sql_error)?;
    let item_count: i64 = row.try_get("item_count").map_err(map_sql_error)?;
    let state_version = result.state_version;
    let event = match command.action {
        auction_application::AuctionLifecycleAction::Preview => (
            "auction.preview_updated",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "preview_revision": preview_revision,
                "item_count": item_count,
                "manual_open": manual_open,
                "state_version": state_version,
            }),
        ),
        auction_application::AuctionLifecycleAction::Open => (
            "auction.opened",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "world_id": world_id,
                "state_version": state_version,
                "preview_revision": preview_revision,
            }),
        ),
        auction_application::AuctionLifecycleAction::Cancel => (
            "auction.cancelled",
            json!({ "auction_id": result.auction_id.as_uuid(), "state_version": state_version }),
        ),
        auction_application::AuctionLifecycleAction::FinalPhase => (
            "auction.final_phase_entered",
            json!({ "auction_id": result.auction_id.as_uuid(), "state_version": state_version }),
        ),
    };
    Ok(event)
}
