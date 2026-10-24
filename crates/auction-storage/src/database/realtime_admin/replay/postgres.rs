//! PostgreSQL realtime failure replay 事务；同事务内锁定 failure 与 stream、追加替代事件，
//! 仅在连续边界越过所选 consumer 的原事件，收口 failure 并写入审计与命令账本。

use auction_application::{
    ApplicationError, PublicRealtimeEvent, RealtimeAdminError, RealtimeReplayCommand,
    RealtimeReplayResult,
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use super::super::common::{admin_error, storage_error};
use super::{REPLAY_COMMAND_KIND, replay_result_from_payload, replay_result_to_payload};
use crate::database::PostgresStore;
use crate::database::audit_health::AuditWriteResult as _;
use crate::database::realtime_consumer::resolve_failure_postgres_tx;

pub(crate) async fn replay_postgres(
    store: &PostgresStore,
    command: RealtimeReplayCommand,
) -> Result<RealtimeReplayResult, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|_| storage_error("打开 PostgreSQL realtime replay 事务失败"))?;
    let result = replay_postgres_inner(&mut transaction, &command).await;
    match result {
        Ok(result) => transaction
            .commit()
            .await
            .map(|_| result)
            .map_err(|_| storage_error("提交 PostgreSQL realtime replay 事务失败")),
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

async fn replay_postgres_inner(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &RealtimeReplayCommand,
) -> Result<RealtimeReplayResult, ApplicationError> {
    let pending_payload = json!({ "status": "pending" });
    let inserted = sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code, result_payload, created_at)
         VALUES ($1, $2, $3, $4, 'pending', $5, $6)
         ON CONFLICT (command_id) DO NOTHING",
    )
    .bind(command.command_id)
    .bind(REPLAY_COMMAND_KIND)
    .bind(command.failure_id)
    .bind(command.request_hash.as_bytes().as_slice())
    .bind(sqlx::types::Json(pending_payload))
    .bind(command.created_at)
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("写入 PostgreSQL realtime replay 命令账本失败"))?;

    if inserted.rows_affected() == 0 {
        let row = sqlx::query(
            "SELECT command_kind, aggregate_id, request_hash, result_code, result_payload
             FROM public.auction_commands
             WHERE command_id = $1
             FOR UPDATE",
        )
        .bind(command.command_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(|_| storage_error("读取 PostgreSQL realtime replay 命令账本失败"))?
        .ok_or_else(|| storage_error("PostgreSQL realtime replay 命令冲突后记录缺失"))?;
        let command_kind: String = row
            .try_get("command_kind")
            .map_err(|_| storage_error("读取 PostgreSQL replay 命令类型失败"))?;
        let aggregate_id: Uuid = row
            .try_get("aggregate_id")
            .map_err(|_| storage_error("读取 PostgreSQL replay 聚合标识失败"))?;
        let request_hash: Vec<u8> = row
            .try_get("request_hash")
            .map_err(|_| storage_error("读取 PostgreSQL replay 请求摘要失败"))?;
        if command_kind != REPLAY_COMMAND_KIND
            || aggregate_id != command.failure_id
            || request_hash != command.request_hash.as_bytes()
        {
            return Err(admin_error(RealtimeAdminError::CommandConflict));
        }
        let result_code: String = row
            .try_get("result_code")
            .map_err(|_| storage_error("读取 PostgreSQL replay 结果代码失败"))?;
        let result_payload: sqlx::types::Json<Value> = row
            .try_get("result_payload")
            .map_err(|_| storage_error("读取 PostgreSQL replay 结果负载失败"))?;
        if result_code == "replayed" {
            return replay_result_from_payload(result_payload.0, command.command_id);
        }
        return Err(storage_error("PostgreSQL realtime replay 命令账本处于未完成状态"));
    }

    let failure = sqlx::query(
        "SELECT failure.consumer_key, failure.resolved_at, failure.replay_event_id,
                original.event_id, original.stream_key, original.stream_sequence,
                original.kind, original.schema_version, original.public_payload,
                original.state_version, original.origin_command_id,
                original.origin_bid_id, original.origin_fact_index, original.created_at
         FROM public.realtime_failures AS failure
         JOIN public.realtime_events AS original ON original.event_id = failure.event_id
         WHERE failure.id = $1
         FOR UPDATE",
    )
    .bind(command.failure_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL realtime failure replay 原记录失败"))?
    .ok_or_else(|| admin_error(RealtimeAdminError::NotFound))?;
    let resolved_at: Option<DateTime<Utc>> = failure
        .try_get("resolved_at")
        .map_err(|_| storage_error("读取 PostgreSQL realtime failure 解决时间失败"))?;
    let existing_replay: Option<Uuid> = failure
        .try_get("replay_event_id")
        .map_err(|_| storage_error("读取 PostgreSQL realtime failure replay 引用失败"))?;
    if resolved_at.is_some() || existing_replay.is_some() {
        return Err(admin_error(RealtimeAdminError::AlreadyResolved));
    }
    let consumer_key: String = failure
        .try_get("consumer_key")
        .map_err(|_| storage_error("读取 PostgreSQL realtime failure consumer 失败"))?;

    let original = PublicRealtimeEvent {
        event_id: failure
            .try_get("event_id")
            .map_err(|_| storage_error("读取 PostgreSQL 原 realtime event ID 失败"))?,
        stream_key: failure
            .try_get("stream_key")
            .map_err(|_| storage_error("读取 PostgreSQL 原 realtime stream 失败"))?,
        stream_sequence: failure
            .try_get("stream_sequence")
            .map_err(|_| storage_error("读取 PostgreSQL 原 realtime 序号失败"))?,
        kind: failure
            .try_get("kind")
            .map_err(|_| storage_error("读取 PostgreSQL 原 realtime 类型失败"))?,
        schema_version: failure
            .try_get("schema_version")
            .map_err(|_| storage_error("读取 PostgreSQL 原 realtime schema 失败"))?,
        payload: failure
            .try_get::<sqlx::types::Json<Value>, _>("public_payload")
            .map_err(|_| storage_error("读取 PostgreSQL 原 realtime 负载失败"))?
            .0,
        created_at: failure
            .try_get("created_at")
            .map_err(|_| storage_error("读取 PostgreSQL 原 realtime 时间失败"))?,
    };
    if original.validate().is_err() {
        return Err(admin_error(RealtimeAdminError::ReplayMismatch));
    }
    let origin_command_id: Option<Uuid> = failure
        .try_get("origin_command_id")
        .map_err(|_| storage_error("读取 PostgreSQL 原 realtime 命令引用失败"))?;
    let origin_bid_id: Option<Uuid> = failure
        .try_get("origin_bid_id")
        .map_err(|_| storage_error("读取 PostgreSQL 原 realtime bid 引用失败"))?;
    let origin_fact_index: Option<i32> = failure
        .try_get("origin_fact_index")
        .map_err(|_| storage_error("读取 PostgreSQL 原 realtime fact 序号失败"))?;
    let state_version: Option<i64> = failure
        .try_get("state_version")
        .map_err(|_| storage_error("读取 PostgreSQL 原 realtime 状态版本失败"))?;

    let stream_sequence: i64 = sqlx::query_scalar(
        "SELECT next_sequence
         FROM public.realtime_streams
         WHERE stream_key = $1
         FOR UPDATE",
    )
    .bind(&original.stream_key)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("锁定 PostgreSQL realtime stream 失败"))?
    .ok_or_else(|| storage_error("PostgreSQL realtime stream 不存在"))?;
    if stream_sequence <= original.stream_sequence {
        return Err(admin_error(RealtimeAdminError::ReplayMismatch));
    }
    let event_id = Uuid::new_v4();
    let replay = PublicRealtimeEvent {
        event_id,
        stream_key: original.stream_key.clone(),
        stream_sequence,
        kind: original.kind.clone(),
        schema_version: command.schema_version,
        payload: command.public_payload.clone(),
        created_at: command.created_at,
    };
    if replay.validate().is_err() {
        return Err(admin_error(RealtimeAdminError::ReplayMismatch));
    }
    sqlx::query(
        "UPDATE public.realtime_streams
         SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
         WHERE stream_key = $1",
    )
    .bind(&replay.stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("推进 PostgreSQL realtime stream 序号失败"))?;
    sqlx::query(
        "INSERT INTO public.realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id, origin_bid_id, origin_fact_index,
             replay_of_event_id, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(replay.event_id)
    .bind(&replay.stream_key)
    .bind(replay.stream_sequence)
    .bind(&replay.kind)
    .bind(replay.schema_version)
    .bind(sqlx::types::Json(&replay.payload))
    .bind(state_version)
    .bind(origin_command_id)
    .bind(origin_bid_id)
    .bind(origin_fact_index)
    .bind(original.event_id)
    .bind(replay.created_at)
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("写入 PostgreSQL realtime replay 事件失败"))?;

    // 只恢复本 failure 绑定的 consumer；其他 consumer 即便失败于同一原事件，也必须
    // 独立证明自己的连续边界。复用事务入口，不能在这里另开连接或嵌套提交。
    resolve_failure_postgres_tx(
        transaction,
        &consumer_key,
        &original.stream_key,
        original.event_id,
        replay.event_id,
        command.created_at,
    )
    .await?;

    let metadata = json!({
        "command_id": command.command_id,
        "failure_id": command.failure_id,
        "original_event_id": original.event_id,
        "replay_event_id": replay.event_id,
        "stream_key": replay.stream_key,
        "stream_sequence": replay.stream_sequence,
        "kind": replay.kind,
        "schema_version": replay.schema_version,
    });
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
             result, request_id, source, metadata)
         VALUES ($1, $2, 'admin', $3, 'realtime.replay', 'realtime_failure', $4,
                 'success', $5, 'admin', $6)",
    )
    .bind(Uuid::new_v4())
    .bind(command.created_at)
    .bind(&command.actor_id)
    .bind(command.failure_id.to_string())
    .bind(command.request_id.as_deref())
    .bind(sqlx::types::Json(metadata))
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(|_| storage_error("写入 PostgreSQL realtime replay audit 失败"))?;

    let result = RealtimeReplayResult {
        command_id: command.command_id,
        event_id: replay.event_id,
        replay_of_event_id: original.event_id,
        stream_key: replay.stream_key,
        stream_sequence: replay.stream_sequence,
        kind: replay.kind,
        schema_version: replay.schema_version,
        created_at: replay.created_at,
    };
    let result_payload = serde_json::to_value(replay_result_to_payload(&result))
        .map_err(|_| storage_error("序列化 PostgreSQL realtime replay 结果失败"))?;
    let updated = sqlx::query(
        "UPDATE public.auction_commands
         SET result_code = 'replayed', result_payload = $2
         WHERE command_id = $1 AND result_code = 'pending'",
    )
    .bind(command.command_id)
    .bind(sqlx::types::Json(result_payload))
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("更新 PostgreSQL realtime replay 命令结果失败"))?;
    if updated.rows_affected() != 1 {
        return Err(storage_error("PostgreSQL realtime replay 命令结果状态异常"));
    }
    Ok(result)
}
