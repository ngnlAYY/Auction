//! SQLite 生命周期审计与 realtime 事件记录。

use super::super::mapping::*;
use super::super::*;

pub(super) async fn append_lifecycle_record_inner(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    result: &AuctionLifecycleResult,
) -> Result<(), CompetitionTransactionError> {
    let metadata = serde_json::to_string(&json!({
        "command_id": command.command_id.as_uuid(),
        "action": command.action.command_kind(),
        "status": auction_status_name(result.status),
        "state_version": result.state_version,
    }))
    .map_err(|_| invalid_record())?;
    {
        let connection = transaction.connection_mut()?;
        sqlx::query(
            "INSERT INTO audit_records
                (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
             VALUES (?, ?, ?, ?, 'auction', ?, 'success', ?, ?, ?)",
        )
        .bind(uuid_blob(Uuid::new_v4()))
        .bind(command.actor.actor_type.database_name())
        .bind(&command.actor.actor_id)
        .bind(command.action.command_kind())
        .bind(result.auction_id.as_uuid().to_string())
        .bind(command.request_id.as_deref())
        .bind(command.actor.actor_type.database_name())
        .bind(metadata)
        .execute(&mut *connection)
        .await
        .record_audit_failure()
        .map_err(map_sql_error)?;
    }

    let (event_kind, payload) =
        lifecycle_event_payload(transaction.connection_mut()?, command, result).await?;
    let payload = serde_json::to_string(&payload).map_err(|_| invalid_record())?;
    let stream_key = format!("auction:{}", result.auction_id.as_uuid());
    let connection = transaction.connection_mut()?;
    sqlx::query(
        "INSERT INTO realtime_streams (stream_key, next_sequence, updated_at)
         VALUES (?, 1, CURRENT_TIMESTAMP)
         ON CONFLICT(stream_key) DO NOTHING",
    )
    .bind(&stream_key)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    let sequence: i64 =
        sqlx::query_scalar("SELECT next_sequence FROM realtime_streams WHERE stream_key = ?")
            .bind(&stream_key)
            .fetch_one(&mut *connection)
            .await
            .map_err(map_sql_error)?;
    sqlx::query(
        "UPDATE realtime_streams
         SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
         WHERE stream_key = ?",
    )
    .bind(&stream_key)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    sqlx::query(
        "INSERT INTO realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id)
         VALUES (?, ?, ?, ?, 1, ?, ?, ?)",
    )
    .bind(uuid_blob(Uuid::new_v4()))
    .bind(&stream_key)
    .bind(sequence)
    .bind(event_kind)
    .bind(payload)
    .bind(i64_version(result.state_version, "生命周期事件状态版本")?)
    .bind(uuid_blob(command.command_id.as_uuid()))
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

async fn lifecycle_event_payload(
    connection: &mut SqliteConnection,
    command: &AuctionLifecycleCommand,
    result: &AuctionLifecycleResult,
) -> Result<(&'static str, Value), CompetitionTransactionError> {
    let row = sqlx::query(
        "SELECT a.world_id,
                COALESCE((SELECT MAX(source_revision)
                          FROM auction_preview_items AS item
                          WHERE item.auction_id = a.id), 0) AS preview_revision,
                (SELECT COUNT(*)
                 FROM auction_preview_items AS item
                 WHERE item.auction_id = a.id) AS item_count
         FROM auctions AS a
         WHERE a.id = ?",
    )
    .bind(uuid_blob(result.auction_id.as_uuid()))
    .fetch_one(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    let world_id: String = row.try_get("world_id").map_err(map_sql_error)?;
    let preview_revision: i64 = row.try_get("preview_revision").map_err(map_sql_error)?;
    let item_count: i64 = row.try_get("item_count").map_err(map_sql_error)?;
    let state_version = result.state_version;
    let event = match command.action {
        auction_application::AuctionLifecycleAction::Preview => (
            "auction.preview_updated",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "preview_revision": preview_revision,
                "item_count": item_count,
                "manual_open": false,
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
