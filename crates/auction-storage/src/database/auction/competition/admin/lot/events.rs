//! Admin lot 审计和 preview realtime 事件写入。

use crate::database::auction::competition::PostgresCompetitionTransaction;
use crate::database::auction::competition::admin::map_sql_error;
use crate::database::operations::audit_health::AuditWriteResult;
use auction_application::admin::{AdminLotSummary, AdminLotUpdateCommand};
use auction_application::core::CompetitionTransactionError;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

pub(super) async fn append_admin_lot_record_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AdminLotUpdateCommand,
    result: &AdminLotSummary,
    changed: bool,
    auction_state_version: i64,
    preview_revision: i64,
) -> Result<(), CompetitionTransactionError> {
    let metadata = json!({
        "command_id": command.command_id.as_uuid(),
        "action": command.action.as_str(),
        "lot_id": result.id.as_uuid(),
        "state_version": result.state_version,
        "changed": changed,
        "preview_revision": preview_revision,
    });
    let transaction = transaction.transaction_mut()?;
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, request_id, source, metadata)
         VALUES ($1, 'admin', $2, $3, 'lot', $4, 'success', $5, 'admin', $6)",
    )
    .bind(Uuid::new_v4())
    .bind(&command.actor_id)
    .bind(format!("lot.{}", command.action.as_str()))
    .bind(result.id.as_uuid().to_string())
    .bind(&command.request_id)
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;

    if !changed {
        return Ok(());
    }

    let event_data = sqlx::query(
        "SELECT a.world_id, a.preview_manual_open,
                (SELECT COUNT(*)::bigint FROM public.auction_preview_items AS item
                 WHERE item.auction_id = a.id) AS item_count
         FROM public.auctions AS a WHERE a.id = $1",
    )
    .bind(result.auction_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let item_count: i64 = event_data.try_get("item_count").map_err(map_sql_error)?;
    let manual_open: bool = event_data.try_get("preview_manual_open").map_err(map_sql_error)?;
    let stream_key = format!("auction:{}", result.auction_id.as_uuid());
    let sequence =
        crate::database::realtime::allocate_stream_sequence(transaction, &stream_key, None)
            .await
            .map_err(map_sql_error)?;
    sqlx::query(
        "INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id)
         VALUES ($1, $2, $3, 'auction.preview_updated', 1, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(&stream_key)
    .bind(sequence)
    .bind(json!({
        "auction_id": result.auction_id.as_uuid(),
        "preview_revision": preview_revision,
        "item_count": item_count,
        "manual_open": manual_open,
        "state_version": auction_state_version,
    }))
    .bind(auction_state_version)
    .bind(command.command_id.as_uuid())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}
