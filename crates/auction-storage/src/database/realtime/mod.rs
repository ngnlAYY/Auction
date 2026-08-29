//! Public 持久化实时事件读取适配器。
//!
//! 事件回放只读取已经提交的 `realtime_events`，不从内存广播器补造消息。按拍卖读取时先
//! 校验公开拍卖边界；遇到历史事件的 kind、schema 或负载字段不符合注册表时停止续读并
//! 返回 `refetch_required`，避免把序号空洞误当成可恢复的增量。

use auction_application::{
    ApplicationError, ApplicationFuture, PublicRealtimeEvent, PublicRealtimePage,
    PublicRealtimeReader, PublicRealtimeRequest, RepositoryError,
};
#[cfg(feature = "sqlite")]
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use super::DatabaseStore;

impl PublicRealtimeReader for DatabaseStore {
    fn list_public_events(
        &self,
        request: PublicRealtimeRequest,
    ) -> ApplicationFuture<'_, Option<PublicRealtimePage>> {
        Box::pin(async move {
            request.validate().map_err(storage_error)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => list_sqlite(store, request).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn list_postgres(
    store: &super::PostgresStore,
    request: PublicRealtimeRequest,
) -> Result<Option<PublicRealtimePage>, ApplicationError> {
    let stream_key = stream_key(request.auction_id);
    let limit =
        request.page_size.checked_add(1).ok_or_else(|| storage_error("实时事件页大小溢出"))?;
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("打开实时事件快照失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置实时事件快照失败"))?;

    if let Some(auction_id) = request.auction_id {
        let visible = sqlx::query(
            "SELECT 1
             FROM public.auctions AS auction
             JOIN public.worlds AS world ON world.id = auction.world_id AND world.enabled
             WHERE auction.id = $1",
        )
        .bind(auction_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| storage_error("校验实时事件拍卖边界失败"))?
        .is_some();
        if !visible {
            transaction.commit().await.map_err(|_| storage_error("提交实时事件边界快照失败"))?;
            return Ok(None);
        }
    }

    let rows = sqlx::query(
        "SELECT event_id, stream_key, stream_sequence, kind, schema_version,
                public_payload, created_at
         FROM public.realtime_events
         WHERE stream_key = $1 AND stream_sequence > $2
         ORDER BY stream_sequence ASC
         LIMIT $3",
    )
    .bind(&stream_key)
    .bind(request.after)
    .bind(limit)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 实时事件失败"))?;
    let has_more = rows_len_is_more(request.page_size, &rows);
    transaction.commit().await.map_err(|_| storage_error("提交实时事件快照失败"))?;

    let mut events = Vec::with_capacity(request.page_size as usize);
    let mut refetch_required = false;
    let mut previous_sequence = request.after;
    for (index, row) in rows.into_iter().enumerate() {
        if index >= request.page_size as usize {
            break;
        }
        let event = PublicRealtimeEvent {
            event_id: row
                .try_get("event_id")
                .map_err(|_| storage_error("读取 PostgreSQL 实时事件标识失败"))?,
            stream_key: row
                .try_get("stream_key")
                .map_err(|_| storage_error("读取 PostgreSQL 实时事件流名失败"))?,
            stream_sequence: row
                .try_get("stream_sequence")
                .map_err(|_| storage_error("读取 PostgreSQL 实时事件序号失败"))?,
            kind: row
                .try_get("kind")
                .map_err(|_| storage_error("读取 PostgreSQL 实时事件类型失败"))?,
            schema_version: row
                .try_get("schema_version")
                .map_err(|_| storage_error("读取 PostgreSQL 实时事件版本失败"))?,
            payload: row
                .try_get::<sqlx::types::Json<Value>, _>("public_payload")
                .map_err(|_| storage_error("读取 PostgreSQL 实时事件负载失败"))?
                .0,
            created_at: row
                .try_get("created_at")
                .map_err(|_| storage_error("读取 PostgreSQL 实时事件时间失败"))?,
        };
        // 旧版页在缺口或损坏投影后仍扫描当前有界页，并让 next_after 越过原始行；浏览器据此
        // 清空局部事实缓存、刷新权威快照，同时不会在每次重连时重复卡在同一坏行。
        if event.stream_key != stream_key || event.stream_sequence <= previous_sequence {
            return Err(storage_error("PostgreSQL 实时事件顺序无效"));
        }
        if previous_sequence.checked_add(1) != Some(event.stream_sequence) {
            refetch_required = true;
        }
        previous_sequence = event.stream_sequence;
        if event.validate().is_err() {
            refetch_required = true;
            continue;
        }
        events.push(event);
    }

    Ok(Some(page(previous_sequence, events, refetch_required, has_more)))
}

#[cfg(feature = "sqlite")]
async fn list_sqlite(
    store: &super::SqliteStore,
    request: PublicRealtimeRequest,
) -> Result<Option<PublicRealtimePage>, ApplicationError> {
    let stream_key = stream_key(request.auction_id);
    let limit =
        request.page_size.checked_add(1).ok_or_else(|| storage_error("实时事件页大小溢出"))?;
    let mut connection =
        store.pool().acquire().await.map_err(|_| storage_error("打开 Lite 实时事件快照失败"))?;

    if let Some(auction_id) = request.auction_id {
        let visible = sqlx::query(
            "SELECT 1
             FROM auctions AS auction
             JOIN worlds AS world ON world.id = auction.world_id AND world.enabled = 1
             WHERE auction.id = ?",
        )
        .bind(uuid_blob(auction_id))
        .fetch_optional(&mut *connection)
        .await
        .map_err(|_| storage_error("校验 Lite 实时事件拍卖边界失败"))?
        .is_some();
        if !visible {
            return Ok(None);
        }
    }

    let rows = sqlx::query(
        "SELECT event_id, stream_key, stream_sequence, kind, schema_version,
                public_payload, created_at
         FROM realtime_events
         WHERE stream_key = ? AND stream_sequence > ?
         ORDER BY stream_sequence ASC
         LIMIT ?",
    )
    .bind(&stream_key)
    .bind(request.after)
    .bind(limit)
    .fetch_all(&mut *connection)
    .await
    .map_err(|_| storage_error("读取 Lite 实时事件失败"))?;

    let has_more = rows_len_is_more(request.page_size, &rows);
    let mut events = Vec::with_capacity(request.page_size as usize);
    let mut refetch_required = false;
    let mut previous_sequence = request.after;
    for (index, row) in rows.into_iter().enumerate() {
        if index >= request.page_size as usize {
            break;
        }
        let event = PublicRealtimeEvent {
            event_id: uuid_from_blob(
                row.try_get("event_id").map_err(|_| storage_error("读取 Lite 实时事件标识失败"))?,
                "Lite 实时事件标识",
            )?,
            stream_key: row
                .try_get("stream_key")
                .map_err(|_| storage_error("读取 Lite 实时事件流名失败"))?,
            stream_sequence: row
                .try_get("stream_sequence")
                .map_err(|_| storage_error("读取 Lite 实时事件序号失败"))?,
            kind: row.try_get("kind").map_err(|_| storage_error("读取 Lite 实时事件类型失败"))?,
            schema_version: row
                .try_get("schema_version")
                .map_err(|_| storage_error("读取 Lite 实时事件版本失败"))?,
            payload: serde_json::from_str(
                &row.try_get::<String, _>("public_payload")
                    .map_err(|_| storage_error("读取 Lite 实时事件负载失败"))?,
            )
            .map_err(|_| storage_error("Lite 实时事件负载不是有效 JSON"))?,
            created_at: parse_timestamp(
                row.try_get("created_at")
                    .map_err(|_| storage_error("读取 Lite 实时事件时间失败"))?,
            )?,
        };
        // Lite 保持同一旧版恢复语义：顺序倒退属于存储损坏，缺口或坏投影则标记 refetch，
        // 继续推进原始 cursor，避免永久重放同一不可公开事件。
        if event.stream_key != stream_key || event.stream_sequence <= previous_sequence {
            return Err(storage_error("Lite 实时事件顺序无效"));
        }
        if previous_sequence.checked_add(1) != Some(event.stream_sequence) {
            refetch_required = true;
        }
        previous_sequence = event.stream_sequence;
        if event.validate().is_err() {
            refetch_required = true;
            continue;
        }
        events.push(event);
    }

    Ok(Some(page(previous_sequence, events, refetch_required, has_more)))
}

fn page(
    next_after: i64,
    events: Vec<PublicRealtimeEvent>,
    refetch_required: bool,
    has_more: bool,
) -> PublicRealtimePage {
    PublicRealtimePage { events, next_after, has_more, refetch_required }
}

fn stream_key(auction_id: Option<Uuid>) -> String {
    auction_id.map_or_else(|| "auction:discovery".to_owned(), |id| format!("auction:{id}"))
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}

fn rows_len_is_more<T>(page_size: i64, rows: &[T]) -> bool {
    rows.len() > page_size as usize
}

#[cfg(feature = "sqlite")]
fn uuid_blob(value: Uuid) -> Vec<u8> {
    value.as_bytes().to_vec()
}

#[cfg(feature = "sqlite")]
fn uuid_from_blob(bytes: Vec<u8>, label: &str) -> Result<Uuid, ApplicationError> {
    Uuid::from_slice(&bytes).map_err(|_| storage_error(label))
}

#[cfg(feature = "sqlite")]
fn parse_timestamp(value: String) -> Result<DateTime<Utc>, ApplicationError> {
    if let Ok(value) = DateTime::parse_from_rfc3339(&value) {
        return Ok(value.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(value) = chrono::NaiveDateTime::parse_from_str(&value, format) {
            return Ok(DateTime::<Utc>::from_naive_utc_and_offset(value, Utc));
        }
    }
    Err(storage_error("Lite 实时事件时间格式无效"))
}
