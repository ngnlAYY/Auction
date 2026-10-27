//! CAS、原始命令结果与追加审计同事务提交；重放先于当前状态门禁。

use auction_application::draft_soft_close::{
    DRAFT_SOFT_CLOSE_COMMAND_KIND, DraftSoftCloseError, DraftSoftCloseRepairResult,
    REPAIRED_SOFT_CLOSE_SECONDS, RepairDraftSoftCloseCommand,
};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use super::{Failure, PostgresStore, read};
use crate::database::audit_health::AuditWriteResult as _;

pub(super) async fn execute(
    store: &PostgresStore,
    command: &RepairDraftSoftCloseCommand,
) -> Result<DraftSoftCloseRepairResult, Failure> {
    let hash = command.request_hash()?.as_bytes();
    let mut transaction = store.pool().begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
    let stored = sqlx::query(
        "SELECT command_kind, aggregate_id, request_hash, result_code, result_payload
         FROM public.auction_commands WHERE command_id = $1",
    )
    .bind(command.command_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some(stored) = stored {
        if stored.try_get::<String, _>("command_kind")? != DRAFT_SOFT_CLOSE_COMMAND_KIND
            || stored.try_get::<Uuid, _>("aggregate_id")? != command.auction_id
            || stored.try_get::<Vec<u8>, _>("request_hash")?.as_slice() != hash.as_slice()
            || stored.try_get::<String, _>("result_code")? != "repaired"
        {
            return Err(DraftSoftCloseError::Conflict.into());
        }
        let result: DraftSoftCloseRepairResult =
            serde_json::from_value(stored.try_get::<Value, _>("result_payload")?)
                .map_err(|_| DraftSoftCloseError::Storage)?;
        if result.auction_id != command.auction_id
            || result.command_id != command.command_id
            || result.previous_version != command.expected_version
            || result.state_version != command.expected_version + 1
            || result.window_seconds != REPAIRED_SOFT_CLOSE_SECONDS
            || result.extension_seconds != REPAIRED_SOFT_CLOSE_SECONDS
        {
            return Err(DraftSoftCloseError::Storage.into());
        }
        transaction.commit().await?;
        return Ok(result);
    }

    read::lock_target(&mut transaction, command.auction_id).await?;
    let snapshot = read::snapshot(&mut transaction, command.auction_id).await?;
    snapshot.authorize(command)?;
    // 仅替换两字段，不重建 JSON，保留所有其他经济、结算、恢复及计划快照。
    let row = sqlx::query(
        "UPDATE public.auctions
         SET soft_close_window_seconds = $3, soft_close_extension_seconds = $3,
             policy_snapshot = jsonb_set(jsonb_set(policy_snapshot,
                 '{soft_close_window_seconds}', to_jsonb($3::bigint), false),
                 '{soft_close_extension_seconds}', to_jsonb($3::bigint), false),
             state_version = state_version + 1,
             updated_at = GREATEST(updated_at, transaction_timestamp())
         WHERE id = $1 AND state_version = $2 AND status = 'draft'
           AND soft_close_window_seconds = 60 AND soft_close_extension_seconds = 60
           AND policy_snapshot = $4
         RETURNING state_version, updated_at",
    )
    .bind(command.auction_id)
    .bind(command.expected_version)
    .bind(REPAIRED_SOFT_CLOSE_SECONDS)
    .bind(&snapshot.policy_snapshot)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(DraftSoftCloseError::Conflict)?;
    let result = DraftSoftCloseRepairResult {
        auction_id: command.auction_id,
        command_id: command.command_id,
        previous_version: command.expected_version,
        state_version: row.try_get("state_version")?,
        window_seconds: REPAIRED_SOFT_CLOSE_SECONDS,
        extension_seconds: REPAIRED_SOFT_CLOSE_SECONDS,
        repaired_at: row.try_get("updated_at")?,
    };
    let payload = serde_json::to_value(&result).map_err(|_| DraftSoftCloseError::Storage)?;
    sqlx::query(
        "INSERT INTO public.auction_commands
             (command_id, command_kind, aggregate_id, request_hash, result_code, result_payload)
         VALUES ($1, $2, $3, $4, 'repaired', $5)",
    )
    .bind(command.command_id)
    .bind(DRAFT_SOFT_CLOSE_COMMAND_KIND)
    .bind(command.auction_id)
    .bind(hash.as_slice())
    .bind(payload)
    .execute(&mut *transaction)
    .await
    .map_err(|error| {
        if error.as_database_error().is_some_and(|error| error.code().as_deref() == Some("23505")) {
            Failure::Application(DraftSoftCloseError::Conflict)
        } else {
            Failure::Sql(error)
        }
    })?;
    sqlx::query(
        "INSERT INTO public.audit_records
             (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
              result, request_id, source, metadata)
         VALUES ($1, $2, 'admin', $3, 'auction.repair_draft_soft_close', 'auction', $4,
                 'success', $5, 'admin', $6)",
    )
    .bind(Uuid::new_v5(&command.command_id, b"repair-draft-soft-close:audit"))
    .bind(result.repaired_at)
    .bind(&command.actor)
    .bind(command.auction_id.to_string())
    .bind(format!("auctionctl:{}", command.command_id))
    .bind(json!({
        "command_id": command.command_id,
        "previous_version": command.expected_version,
        "state_version": result.state_version,
        "previous_window_seconds": snapshot.window_seconds,
        "previous_extension_seconds": snapshot.extension_seconds,
        "window_seconds": result.window_seconds,
        "extension_seconds": result.extension_seconds,
        "confirmation": command.confirmation,
    }))
    .execute(&mut *transaction)
    .await
    .record_audit_failure()?;
    transaction.commit().await?;
    Ok(result)
}
