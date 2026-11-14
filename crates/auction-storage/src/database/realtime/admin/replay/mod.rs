//! Admin realtime 重放命令校验与 PostgreSQL 事务适配。

use auction_application::core::ApplicationError;
use auction_application::realtime::RealtimeReplayResult;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

mod postgres;

pub(super) use postgres::replay_postgres;

const REPLAY_COMMAND_KIND: &str = "replay_realtime_failure";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct ReplayLedgerPayload {
    command_id: Uuid,
    event_id: Uuid,
    replay_of_event_id: Uuid,
    stream_key: String,
    stream_sequence: i64,
    kind: String,
    schema_version: i32,
    created_at: DateTime<Utc>,
}

pub(super) fn replay_result_to_payload(result: &RealtimeReplayResult) -> ReplayLedgerPayload {
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

pub(super) fn replay_result_from_payload(
    payload: Value,
    command_id: Uuid,
) -> Result<RealtimeReplayResult, ApplicationError> {
    let payload: ReplayLedgerPayload = serde_json::from_value(payload)
        .map_err(|_| super::errors::storage_error("realtime replay 账本结果无效"))?;
    if payload.command_id != command_id
        || payload.event_id.is_nil()
        || payload.replay_of_event_id.is_nil()
        || payload.stream_key.trim().is_empty()
        || payload.stream_sequence <= 0
        || payload.kind.trim().is_empty()
        || payload.schema_version <= 0
        || payload.created_at.timestamp_micros() == 0
    {
        return Err(super::errors::storage_error("realtime replay 账本结果字段无效"));
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
