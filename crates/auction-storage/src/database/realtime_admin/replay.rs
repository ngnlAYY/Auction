//! Realtime Admin replay 的 PostgreSQL/SQLite 事务实现。
//!
//! replay 必须在本地数据库事务中同时完成命令账本、事件追加、failure 解决和审计写入；
//! 原事件只读保留，替代事件通过 replay 关系指向原事实。

use auction_application::{
    ApplicationError, PublicRealtimeEvent, RealtimeAdminError, RealtimeReplayCommand,
    RealtimeReplayResult,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
#[cfg(feature = "sqlite")]
use sqlx::SqliteConnection;
use uuid::Uuid;

#[cfg(feature = "sqlite")]
use super::common::{
    SqliteRealtimeAdminTransaction, optional_uuid_from_blob, parse_timestamp, uuid_blob,
    uuid_from_blob,
};
use super::common::{admin_error, storage_error};
use crate::database::audit_health::AuditWriteResult as _;

const REPLAY_COMMAND_KIND: &str = "replay_realtime_failure";

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ReplayLedgerPayload {
    command_id: Uuid,
    event_id: Uuid,
    replay_of_event_id: Uuid,
    stream_key: String,
    stream_sequence: i64,
    kind: String,
    schema_version: i32,
    created_at: DateTime<Utc>,
}

#[cfg(feature = "postgres")]
pub(super) async fn replay_postgres(
    store: &super::super::PostgresStore,
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

#[cfg(feature = "postgres")]
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
    .bind(command.request_hash.as_bytes().to_vec())
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
        "SELECT failure.resolved_at, failure.replay_event_id,
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
    .bind(sqlx::types::Json(replay.payload.clone()))
    .bind(state_version)
    .bind(origin_command_id)
    .bind(origin_bid_id)
    .bind(origin_fact_index)
    .bind(original.event_id)
    .bind(replay.created_at)
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("写入 PostgreSQL realtime replay 事件失败"))?;

    let resolved_at = command.created_at;
    let updated = sqlx::query(
        "UPDATE public.realtime_failures
         SET resolved_at = $2, replay_event_id = $3
         WHERE id = $1 AND resolved_at IS NULL AND replay_event_id IS NULL",
    )
    .bind(command.failure_id)
    .bind(resolved_at)
    .bind(replay.event_id)
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("解决 PostgreSQL realtime failure 失败"))?;
    if updated.rows_affected() != 1 {
        return Err(admin_error(RealtimeAdminError::AlreadyResolved));
    }

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

#[cfg(feature = "sqlite")]
pub(super) async fn replay_sqlite(
    store: &super::super::SqliteStore,
    command: RealtimeReplayCommand,
) -> Result<RealtimeReplayResult, ApplicationError> {
    let connection = store
        .pool()
        .acquire()
        .await
        .map_err(|_| storage_error("打开 Lite realtime replay 事务失败"))?;
    let mut transaction = SqliteRealtimeAdminTransaction::begin(connection).await?;
    let result = replay_sqlite_inner(transaction.connection_mut()?, &command).await;
    match result {
        Ok(result) => transaction.commit().await.map(|_| result),
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

#[cfg(feature = "sqlite")]
async fn replay_sqlite_inner(
    connection: &mut SqliteConnection,
    command: &RealtimeReplayCommand,
) -> Result<RealtimeReplayResult, ApplicationError> {
    let pending_payload = serde_json::to_string(&json!({ "status": "pending" }))
        .map_err(|_| storage_error("序列化 Lite realtime replay 待处理结果失败"))?;
    let inserted = sqlx::query(
        "INSERT INTO auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code, result_payload, created_at)
         VALUES (?, ?, ?, ?, 'pending', ?, ?)
         ON CONFLICT(command_id) DO NOTHING",
    )
    .bind(uuid_blob(command.command_id))
    .bind(REPLAY_COMMAND_KIND)
    .bind(uuid_blob(command.failure_id))
    .bind(command.request_hash.as_bytes().to_vec())
    .bind(&pending_payload)
    .bind(command.created_at.to_rfc3339())
    .execute(&mut *connection)
    .await
    .map_err(|_| storage_error("写入 Lite realtime replay 命令账本失败"))?;

    if inserted.rows_affected() == 0 {
        let row = sqlx::query(
            "SELECT command_kind, aggregate_id, request_hash, result_code, result_payload
             FROM auction_commands WHERE command_id = ?",
        )
        .bind(uuid_blob(command.command_id))
        .fetch_optional(&mut *connection)
        .await
        .map_err(|_| storage_error("读取 Lite realtime replay 命令账本失败"))?
        .ok_or_else(|| storage_error("Lite realtime replay 命令冲突后记录缺失"))?;
        let command_kind: String = row
            .try_get("command_kind")
            .map_err(|_| storage_error("读取 Lite replay 命令类型失败"))?;
        let aggregate_id = uuid_from_blob(
            row.try_get("aggregate_id")
                .map_err(|_| storage_error("读取 Lite replay 聚合标识失败"))?,
            "Lite replay 聚合标识",
        )?;
        let request_hash: Vec<u8> = row
            .try_get("request_hash")
            .map_err(|_| storage_error("读取 Lite replay 请求摘要失败"))?;
        if command_kind != REPLAY_COMMAND_KIND
            || aggregate_id != command.failure_id
            || request_hash != command.request_hash.as_bytes()
        {
            return Err(admin_error(RealtimeAdminError::CommandConflict));
        }
        let result_code: String = row
            .try_get("result_code")
            .map_err(|_| storage_error("读取 Lite replay 结果代码失败"))?;
        let result_payload: String = row
            .try_get("result_payload")
            .map_err(|_| storage_error("读取 Lite replay 结果负载失败"))?;
        if result_code == "replayed" {
            let payload: Value = serde_json::from_str(&result_payload)
                .map_err(|_| storage_error("Lite realtime replay 结果负载无效"))?;
            return replay_result_from_payload(payload, command.command_id);
        }
        return Err(storage_error("Lite realtime replay 命令账本处于未完成状态"));
    }

    let failure = sqlx::query(
        "SELECT failure.resolved_at, failure.replay_event_id,
                original.event_id, original.stream_key, original.stream_sequence,
                original.kind, original.schema_version, original.public_payload,
                original.state_version, original.origin_command_id,
                original.origin_bid_id, original.origin_fact_index, original.created_at
         FROM realtime_failures AS failure
         JOIN realtime_events AS original ON original.event_id = failure.event_id
         WHERE failure.id = ?",
    )
    .bind(uuid_blob(command.failure_id))
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| storage_error("读取 Lite realtime failure replay 原记录失败"))?
    .ok_or_else(|| admin_error(RealtimeAdminError::NotFound))?;
    let resolved_at: Option<String> = failure
        .try_get("resolved_at")
        .map_err(|_| storage_error("读取 Lite realtime failure 解决时间失败"))?;
    let existing_replay = failure
        .try_get::<Option<Vec<u8>>, _>("replay_event_id")
        .map_err(|_| storage_error("读取 Lite realtime failure replay 引用失败"))?
        .map(|value| uuid_from_blob(value, "Lite realtime replay 引用"))
        .transpose()?;
    if resolved_at.is_some() || existing_replay.is_some() {
        return Err(admin_error(RealtimeAdminError::AlreadyResolved));
    }
    let original_event_id = uuid_from_blob(
        failure
            .try_get("event_id")
            .map_err(|_| storage_error("读取 Lite 原 realtime event ID 失败"))?,
        "Lite 原 realtime event ID",
    )?;
    let original_stream: String = failure
        .try_get("stream_key")
        .map_err(|_| storage_error("读取 Lite 原 realtime stream 失败"))?;
    let original = PublicRealtimeEvent {
        event_id: original_event_id,
        stream_key: original_stream,
        stream_sequence: failure
            .try_get("stream_sequence")
            .map_err(|_| storage_error("读取 Lite 原 realtime 序号失败"))?,
        kind: failure
            .try_get("kind")
            .map_err(|_| storage_error("读取 Lite 原 realtime 类型失败"))?,
        schema_version: failure
            .try_get("schema_version")
            .map_err(|_| storage_error("读取 Lite 原 realtime schema 失败"))?,
        payload: serde_json::from_str(
            &failure
                .try_get::<String, _>("public_payload")
                .map_err(|_| storage_error("读取 Lite 原 realtime 负载失败"))?,
        )
        .map_err(|_| storage_error("Lite 原 realtime 负载不是有效 JSON"))?,
        created_at: parse_timestamp(
            failure
                .try_get("created_at")
                .map_err(|_| storage_error("读取 Lite 原 realtime 时间失败"))?,
        )?,
    };
    if original.validate().is_err() {
        return Err(admin_error(RealtimeAdminError::ReplayMismatch));
    }
    let origin_command_id = optional_uuid_from_blob(
        failure
            .try_get("origin_command_id")
            .map_err(|_| storage_error("读取 Lite 原 realtime 命令引用失败"))?,
        "Lite 原 realtime 命令引用",
    )?;
    let origin_bid_id = optional_uuid_from_blob(
        failure
            .try_get("origin_bid_id")
            .map_err(|_| storage_error("读取 Lite 原 realtime bid 引用失败"))?,
        "Lite 原 realtime bid 引用",
    )?;
    let origin_fact_index: Option<i32> = failure
        .try_get("origin_fact_index")
        .map_err(|_| storage_error("读取 Lite 原 realtime fact 序号失败"))?;
    let state_version: Option<i64> = failure
        .try_get("state_version")
        .map_err(|_| storage_error("读取 Lite 原 realtime 状态版本失败"))?;

    let stream_sequence: i64 =
        sqlx::query_scalar("SELECT next_sequence FROM realtime_streams WHERE stream_key = ?")
            .bind(&original.stream_key)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|_| storage_error("读取 Lite realtime stream 序号失败"))?
            .ok_or_else(|| storage_error("Lite realtime stream 不存在"))?;
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
        "UPDATE realtime_streams
         SET next_sequence = next_sequence + 1, updated_at = CURRENT_TIMESTAMP
         WHERE stream_key = ?",
    )
    .bind(&replay.stream_key)
    .execute(&mut *connection)
    .await
    .map_err(|_| storage_error("推进 Lite realtime stream 序号失败"))?;

    // Lite 表的早期 schema 对 origin bid fact 使用全局唯一约束；replay 不是新的事实，
    // 因此保留公共事件关系但不重复占用该内部事实引用。
    let (
        replay_origin_command_id,
        replay_origin_bid_id,
        replay_origin_fact_index,
        replay_state_version,
    ) = if replay.kind == "lot.bid_accepted" {
        (None, None, None, None)
    } else {
        (origin_command_id, origin_bid_id, origin_fact_index, state_version)
    };
    sqlx::query(
        "INSERT INTO realtime_events
            (event_id, stream_key, stream_sequence, kind, schema_version, public_payload,
             state_version, origin_command_id, origin_bid_id, origin_fact_index,
             replay_of_event_id, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_blob(replay.event_id))
    .bind(&replay.stream_key)
    .bind(replay.stream_sequence)
    .bind(&replay.kind)
    .bind(replay.schema_version)
    .bind(replay.payload.to_string())
    .bind(replay_state_version)
    .bind(replay_origin_command_id.map(uuid_blob))
    .bind(replay_origin_bid_id.map(uuid_blob))
    .bind(replay_origin_fact_index)
    .bind(uuid_blob(original.event_id))
    .bind(replay.created_at.to_rfc3339())
    .execute(&mut *connection)
    .await
    .map_err(|_| storage_error("写入 Lite realtime replay 事件失败"))?;

    sqlx::query(
        "UPDATE realtime_failures
         SET resolved_at = ?, replay_event_id = ?
         WHERE id = ? AND resolved_at IS NULL AND replay_event_id IS NULL",
    )
    .bind(command.created_at.to_rfc3339())
    .bind(uuid_blob(replay.event_id))
    .bind(uuid_blob(command.failure_id))
    .execute(&mut *connection)
    .await
    .map_err(|_| storage_error("解决 Lite realtime failure 失败"))?;
    let updated: i64 = sqlx::query_scalar("SELECT changes()")
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| storage_error("读取 Lite realtime failure 更新结果失败"))?;
    if updated != 1 {
        return Err(admin_error(RealtimeAdminError::AlreadyResolved));
    }

    let metadata = serde_json::to_string(&json!({
        "command_id": command.command_id,
        "failure_id": command.failure_id,
        "original_event_id": original.event_id,
        "replay_event_id": replay.event_id,
        "stream_key": replay.stream_key,
        "stream_sequence": replay.stream_sequence,
        "kind": replay.kind,
        "schema_version": replay.schema_version,
    }))
    .map_err(|_| storage_error("序列化 Lite realtime replay audit 失败"))?;
    sqlx::query(
        "INSERT INTO audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
             result, request_id, source, metadata)
         VALUES (?, ?, 'admin', ?, 'realtime.replay', 'realtime_failure', ?,
                 'success', ?, ?)",
    )
    .bind(uuid_blob(Uuid::new_v4()))
    .bind(command.created_at.to_rfc3339())
    .bind(&command.actor_id)
    .bind(command.failure_id.to_string())
    .bind(command.request_id.as_deref())
    .bind(metadata)
    .execute(&mut *connection)
    .await
    .record_audit_failure()
    .map_err(|_| storage_error("写入 Lite realtime replay audit 失败"))?;

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
    let result_payload = serde_json::to_string(&replay_result_to_payload(&result))
        .map_err(|_| storage_error("序列化 Lite realtime replay 结果失败"))?;
    let updated = sqlx::query(
        "UPDATE auction_commands
         SET result_code = 'replayed', result_payload = ?
         WHERE command_id = ? AND result_code = 'pending'",
    )
    .bind(&result_payload)
    .bind(uuid_blob(command.command_id))
    .execute(&mut *connection)
    .await
    .map_err(|_| storage_error("更新 Lite realtime replay 命令结果失败"))?;
    if updated.rows_affected() != 1 {
        return Err(storage_error("Lite realtime replay 命令结果状态异常"));
    }
    Ok(result)
}

fn replay_result_to_payload(result: &RealtimeReplayResult) -> ReplayLedgerPayload {
    ReplayLedgerPayload {
        command_id: result.command_id,
        event_id: result.event_id,
        replay_of_event_id: result.replay_of_event_id,
        stream_key: result.stream_key.clone(),
        stream_sequence: result.stream_sequence,
        kind: result.kind.clone(),
        schema_version: result.schema_version,
        created_at: result.created_at,
    }
}

fn replay_result_from_payload(
    payload: Value,
    command_id: Uuid,
) -> Result<RealtimeReplayResult, ApplicationError> {
    let payload: ReplayLedgerPayload = serde_json::from_value(payload)
        .map_err(|_| storage_error("realtime replay 账本结果无效"))?;
    if payload.command_id != command_id
        || payload.event_id.is_nil()
        || payload.replay_of_event_id.is_nil()
        || payload.stream_key.trim().is_empty()
        || payload.stream_sequence <= 0
        || payload.kind.trim().is_empty()
        || payload.schema_version <= 0
        || payload.created_at.timestamp_micros() == 0
    {
        return Err(storage_error("realtime replay 账本结果字段无效"));
    }
    Ok(RealtimeReplayResult {
        command_id: payload.command_id,
        event_id: payload.event_id,
        replay_of_event_id: payload.replay_of_event_id,
        stream_key: payload.stream_key,
        stream_sequence: payload.stream_sequence,
        kind: payload.kind,
        schema_version: payload.schema_version,
        created_at: payload.created_at,
    })
}
