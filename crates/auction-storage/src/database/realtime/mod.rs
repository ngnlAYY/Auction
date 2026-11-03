//! Public 持久化实时事件读取适配器；回放只读已提交的 `realtime_events`，不从内存广播器补造消息。
//! 读取先校验公开边界；历史事件 kind/schema/负载不符注册表时停止续读并返回 `refetch_required`。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use auction_application::{
    ApplicationError, ApplicationFuture, PublicRealtimeEvent, PublicRealtimePage,
    PublicRealtimeReader, PublicRealtimeRequest,
};
#[cfg(feature = "postgres")]
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use super::DatabaseStore;
use crate::database::storage_error;

mod sequence;

pub(crate) use sequence::allocate_stream_sequence;

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
    let mut transaction = store
        .pool()
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ")
        .await
        .map_err(|error| storage_error_from(error, "打开实时事件快照失败"))?;

    if let Some(auction_id) = request.auction_id {
        let visible = sqlx::query(
            "SELECT 1
             FROM public.auctions AS auction
             JOIN public.worlds AS world ON world.id = auction.world_id
             WHERE auction.id = $1
               AND (world.enabled OR auction.status = 'settled')",
        )
        .bind(auction_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| storage_error_from(error, "校验实时事件拍卖边界失败"))?
        .is_some();
        if !visible {
            transaction
                .commit()
                .await
                .map_err(|error| storage_error_from(error, "提交实时事件边界快照失败"))?;
            return Ok(None);
        }
    }

    let rows = sqlx::query(
        "SELECT event.event_id, event.stream_key, event.stream_sequence, event.kind,
                event.schema_version, event.public_payload, event.created_at,
                CASE
                    WHEN event.stream_key <> 'auction:discovery' THEN TRUE
                    ELSE EXISTS (
                        SELECT 1
                        FROM public.auctions AS auction
                        JOIN public.worlds AS world ON world.id = auction.world_id
                        WHERE auction.id::text = (event.public_payload ->> 'auction_id')
                          AND auction.world_id = (event.public_payload ->> 'world_id')
                          AND (world.enabled OR auction.status = 'settled')
                    )
                END AS public_visible
         FROM public.realtime_events AS event
         WHERE event.stream_key = $1 AND event.stream_sequence > $2
         ORDER BY event.stream_sequence ASC
         LIMIT $3",
    )
    .bind(&stream_key)
    .bind(request.after)
    .bind(limit)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL 实时事件失败"))?;
    let has_more = rows_len_is_more(request.page_size, &rows);
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交实时事件快照失败"))?;

    let mut events = Vec::with_capacity(rows.len().min(request.page_size as usize));
    let mut refetch_required = false;
    let mut previous_sequence = request.after;
    for (index, row) in rows.into_iter().enumerate() {
        if index >= request.page_size as usize {
            break;
        }
        let public_visible: bool = row
            .try_get("public_visible")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL 实时事件公开边界失败"))?;
        let event = PublicRealtimeEvent {
            event_id: row
                .try_get("event_id")
                .map_err(|error| storage_error_from(error, "读取 PostgreSQL 实时事件标识失败"))?,
            stream_key: row
                .try_get("stream_key")
                .map_err(|error| storage_error_from(error, "读取 PostgreSQL 实时事件流名失败"))?,
            stream_sequence: row
                .try_get("stream_sequence")
                .map_err(|error| storage_error_from(error, "读取 PostgreSQL 实时事件序号失败"))?,
            kind: row
                .try_get("kind")
                .map_err(|error| storage_error_from(error, "读取 PostgreSQL 实时事件类型失败"))?,
            schema_version: row
                .try_get("schema_version")
                .map_err(|error| storage_error_from(error, "读取 PostgreSQL 实时事件版本失败"))?,
            payload: row
                .try_get::<sqlx::types::Json<Value>, _>("public_payload")
                .map_err(|error| storage_error_from(error, "读取 PostgreSQL 实时事件负载失败"))?
                .0,
            created_at: row
                .try_get("created_at")
                .map_err(|error| storage_error_from(error, "读取 PostgreSQL 实时事件时间失败"))?,
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
        if !public_visible {
            // 发现流是跨世界共享序列；隐藏关闭世界的非终态事件时仍推进原始游标，并要求
            // 客户端回读权威 current 快照，避免在同一不可见事件上永久重连。
            refetch_required = true;
            continue;
        }
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

fn rows_len_is_more<T>(page_size: i64, rows: &[T]) -> bool {
    rows.len() > page_size as usize
}
