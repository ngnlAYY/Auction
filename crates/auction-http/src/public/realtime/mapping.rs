//! Public 实时事件页校验与安全 DTO 映射。

use auction_application::realtime::{PublicRealtimeEvent, PublicRealtimePage};
use auction_contracts::{PublicRealtimeEventResponse, PublicRealtimeResponse};
use axum::extract::ws::Utf8Bytes;
use uuid::Uuid;

use super::target::auction_stream_key;
use crate::mapping::public_timestamp;

pub(super) fn valid_page(page: &PublicRealtimePage, auction_id: Uuid, after: i64) -> bool {
    let expected_stream = auction_stream_key(auction_id);
    if page.next_after < after {
        return false;
    }
    let mut previous = after;
    for event in &page.events {
        if event.validate().is_err()
            || event.stream_key != expected_stream
            || event.stream_sequence <= previous
        {
            return false;
        }
        previous = event.stream_sequence;
    }
    page.next_after >= previous
}

/// 映射旧版单拍卖事件页。`auction.created` 不属于单拍卖回放合同；这里隐藏事件但保留底层
/// cursor fence。
pub(super) fn map_auction_event_page(
    page: PublicRealtimePage,
    auction_id: Uuid,
) -> PublicRealtimeResponse {
    PublicRealtimeResponse {
        auction_id: auction_id.to_string(),
        events: page
            .events
            .into_iter()
            .filter(|event| event.kind != "auction.created")
            .map(map_event)
            .collect(),
        next_after: page.next_after,
        has_more: page.has_more,
        refetch_required: page.refetch_required,
    }
}

/// 将已通过注册表校验的事件编码为 WebSocket v1 文本帧；Hub 每个事件只编码一次，
/// 同一 stream 的全部连接共享同一份引用计数正文。
pub(crate) fn encode_websocket_frame(
    event: PublicRealtimeEvent,
) -> Result<Utf8Bytes, serde_json::Error> {
    serde_json::to_string(&map_event(event)).map(Utf8Bytes::from)
}

/// 映射旧版 WebSocket 单事件信封；`stream_key` 是内部路由字段，不进入公开 v1 wire。
pub(super) fn map_event(event: PublicRealtimeEvent) -> PublicRealtimeEventResponse {
    PublicRealtimeEventResponse {
        event_id: event.event_id.to_string(),
        stream_sequence: event.stream_sequence,
        kind: event.kind,
        schema_version: event.schema_version,
        payload: event.payload,
        created_at: public_timestamp(event.created_at),
    }
}
