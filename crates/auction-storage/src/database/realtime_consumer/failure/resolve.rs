//! Realtime consumer failure 的 replay 收口；事务入口供 Admin 在同一事务内原子追加替代事件并越过原失败。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use auction_application::ApplicationError;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::super::common::storage_error;

/// 复用调用方事务，先锁 failure 再锁对应 consumer 游标；只越过原事件，不跳到替代事件。
/// 重复绑定同一 replay 仍复核游标边界，可收口旧版仅标记 resolved、未推进游标的窗口。
#[cfg(feature = "postgres")]
pub(crate) async fn resolve_failure_postgres_tx(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    consumer_key: &str,
    stream_key: &str,
    event_id: Uuid,
    replay_event_id: Uuid,
    resolved_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let failure = sqlx::query(
        "SELECT original.stream_key, original.stream_sequence,
                failure.resolved_at, failure.replay_event_id
         FROM public.realtime_failures AS failure
         JOIN public.realtime_events AS original ON original.event_id = failure.event_id
         WHERE failure.consumer_key = $1 AND failure.event_id = $2
         FOR UPDATE",
    )
    .bind(consumer_key)
    .bind(event_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL 实时失败记录失败"))?;
    let Some(failure) = failure else {
        return Err(storage_error("PostgreSQL 实时失败记录不存在"));
    };
    let original_stream: String = failure
        .try_get("stream_key")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 原实时 stream 失败"))?;
    let original_sequence: i64 = failure
        .try_get("stream_sequence")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 原实时序号失败"))?;
    if original_stream != stream_key {
        return Err(storage_error("PostgreSQL 原实时事件不属于指定 stream"));
    }
    let existing_replay: Option<Uuid> = failure
        .try_get("replay_event_id")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 重放事件引用失败"))?;
    let already_resolved: Option<DateTime<Utc>> = failure
        .try_get("resolved_at")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 失败解决时间失败"))?;
    if existing_replay.is_some_and(|existing_replay| existing_replay != replay_event_id) {
        return Err(storage_error("PostgreSQL 实时失败记录已有其他重放事件"));
    }
    if existing_replay.is_none() && already_resolved.is_some() {
        return Err(storage_error("PostgreSQL 实时失败记录已经通过正常确认解决"));
    }
    let replay = sqlx::query(
        "SELECT stream_key, stream_sequence, replay_of_event_id
         FROM public.realtime_events WHERE event_id = $1",
    )
    .bind(replay_event_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL 重放事件失败"))?
    .ok_or_else(|| storage_error("PostgreSQL 重放事件不存在"))?;
    let replay_stream: String = replay
        .try_get("stream_key")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 重放 stream 失败"))?;
    let replay_sequence: i64 = replay
        .try_get("stream_sequence")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 重放序号失败"))?;
    let replay_of_event_id: Option<Uuid> = replay
        .try_get("replay_of_event_id")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL 重放原事件引用失败"))?;
    if replay_stream != stream_key
        || replay_sequence <= original_sequence
        || replay_of_event_id != Some(event_id)
    {
        return Err(storage_error("PostgreSQL 重放事件必须位于同一 stream 的后续序号"));
    }
    // Admin 不持有 consumer 的 session advisory lock。先物化游标再锁行，避免不存在的
    // 游标与正在首次确认的 owner 并发插入时，以陈旧的零值覆盖已提交的消费进度。
    sqlx::query(
        "INSERT INTO public.realtime_consumer_offsets
            (consumer_key, stream_key, last_sequence, updated_at)
         VALUES ($1, $2, 0, CURRENT_TIMESTAMP)
         ON CONFLICT (consumer_key, stream_key) DO NOTHING",
    )
    .bind(consumer_key)
    .bind(stream_key)
    .execute(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "初始化 PostgreSQL 重放消费游标失败"))?;
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT last_sequence
         FROM public.realtime_consumer_offsets
         WHERE consumer_key = $1 AND stream_key = $2
         FOR UPDATE",
    )
    .bind(consumer_key)
    .bind(stream_key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "锁定 PostgreSQL 重放消费游标失败"))?;
    let predecessor = original_sequence
        .checked_sub(1)
        .ok_or_else(|| storage_error("PostgreSQL 原实时序号无效"))?;
    if current < predecessor {
        return Err(storage_error("PostgreSQL 实时游标尚未到达失败事件前一序号"));
    }
    if current == predecessor {
        sqlx::query(
            "UPDATE public.realtime_consumer_offsets
             SET last_sequence = $3, updated_at = CURRENT_TIMESTAMP
             WHERE consumer_key = $1 AND stream_key = $2",
        )
        .bind(consumer_key)
        .bind(stream_key)
        .bind(original_sequence)
        .execute(&mut **transaction)
        .await
        .map_err(|error| storage_error_from(error, "推进 PostgreSQL 重放消费游标失败"))?;
    }
    sqlx::query(
        "UPDATE public.realtime_failures
         SET resolved_at = COALESCE(resolved_at, GREATEST(failed_at, $3)), replay_event_id = $4
         WHERE consumer_key = $1 AND event_id = $2",
    )
    .bind(consumer_key)
    .bind(event_id)
    .bind(resolved_at)
    .bind(replay_event_id)
    .execute(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "解决 PostgreSQL 实时重放失败记录失败"))?;
    Ok(())
}
