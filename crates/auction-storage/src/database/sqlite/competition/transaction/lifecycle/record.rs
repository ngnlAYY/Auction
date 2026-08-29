//! SQLite 拍卖生命周期审计与实时事件记录。

use crate::database::lifecycle_audit::{LifecycleAuditRecord, lifecycle_parent_audit_record};

use super::super::mapping::*;
use super::super::*;

pub(super) async fn append_lifecycle_record_inner(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
    context: &AuctionLifecycleContext,
    plan: &AuctionLifecyclePlan,
    result: &AuctionLifecycleResult,
    write_summary: AuctionLifecycleWriteSummary,
) -> Result<(), CompetitionTransactionError> {
    let preview_item_count = preview_item_count(transaction, command).await?;
    let record = lifecycle_parent_audit_record(
        command,
        context,
        plan,
        result,
        write_summary,
        preview_item_count,
    )?;
    insert_lifecycle_audit(transaction, record).await?;

    let Some((event_kind, payload)) =
        lifecycle_event_payload(command, context, plan, result, preview_item_count)?
    else {
        return Ok(());
    };
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

/// SQLite Lite 与 PostgreSQL 使用同一生命周期审计记录结构。
pub(super) async fn insert_lifecycle_audit(
    transaction: &mut SqliteCompetitionTransaction,
    record: LifecycleAuditRecord,
) -> Result<(), CompetitionTransactionError> {
    let metadata = serde_json::to_string(&record.metadata).map_err(|_| invalid_record())?;
    let connection = transaction.connection_mut()?;
    sqlx::query(
        "INSERT INTO audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             request_id, source, metadata)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'success', NULL, ?, ?)",
    )
    .bind(uuid_blob(Uuid::new_v4()))
    .bind(sqlite_timestamp(record.occurred_at))
    .bind(record.actor_type)
    .bind(record.actor_id)
    .bind(record.action)
    .bind(record.entity_type)
    .bind(record.entity_id)
    .bind(record.source)
    .bind(metadata)
    .execute(&mut *connection)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}

async fn preview_item_count(
    transaction: &mut SqliteCompetitionTransaction,
    command: &AuctionLifecycleCommand,
) -> Result<Option<u64>, CompetitionTransactionError> {
    if command.action != auction_application::AuctionLifecycleAction::Preview {
        return Ok(None);
    }
    let connection = transaction.connection_mut()?;
    let item_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM auction_preview_items
         WHERE auction_id = ?",
    )
    .bind(uuid_blob(command.auction_id.as_uuid()))
    .fetch_one(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    u64::try_from(item_count).map(Some).map_err(|_| invalid_state("Lite 预览物化条目数量无效"))
}

fn lifecycle_event_payload(
    command: &AuctionLifecycleCommand,
    context: &AuctionLifecycleContext,
    plan: &AuctionLifecyclePlan,
    result: &AuctionLifecycleResult,
    preview_item_count: Option<u64>,
) -> Result<Option<(&'static str, Value)>, CompetitionTransactionError> {
    let event = match &plan.effect {
        AuctionLifecycleEffect::Preview { manual, .. } => Some((
            "auction.preview_updated",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "preview_revision": context.preview_revision,
                "item_count": preview_item_count
                    .ok_or_else(|| invalid_state("Lite 预览实时事件缺少物化条目数量"))?,
                "manual_open": context.preview_manual_open || *manual,
                "state_version": result.state_version,
            }),
        )),
        AuctionLifecycleEffect::Open { .. } => Some((
            "auction.opened",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "world_id": &context.world_id,
                "state_version": result.state_version,
                "preview_revision": context.preview_revision,
            }),
        )),
        AuctionLifecycleEffect::Cancel if context.auction.status == AuctionStatus::Draft => None,
        AuctionLifecycleEffect::Cancel => Some((
            "auction.cancelled",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "state_version": result.state_version,
            }),
        )),
        AuctionLifecycleEffect::FinalPhase { .. } => Some((
            "auction.final_phase_entered",
            json!({
                "auction_id": result.auction_id.as_uuid(),
                "state_version": result.state_version,
            }),
        )),
    };
    if plan.effect.action() != command.action {
        return Err(invalid_state("Lite 生命周期实时事件与命令动作不一致"));
    }
    Ok(event)
}
