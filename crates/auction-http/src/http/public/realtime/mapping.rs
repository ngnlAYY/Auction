//! Public 实时事件页校验与安全 DTO 映射。

use auction_application::{PublicRealtimeEvent, PublicRealtimePage};
use auction_contracts::{
    PublicRealtimeDiscoveryResponse, PublicRealtimeEventResponse, PublicRealtimeResponse,
};
use uuid::Uuid;

use super::super::public_timestamp;

pub(super) fn valid_page(page: &PublicRealtimePage, auction_id: Option<Uuid>, after: i64) -> bool {
    let expected_stream = auction_id.map_or_else(
        || "auction:discovery".to_owned(),
        |auction_id| format!("auction:{auction_id}"),
    );
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

/// 映射旧版单拍卖事件页。`auction.created` 只服务发现流；这里隐藏事件但保留底层 cursor fence。
pub(super) fn map_auction_page(
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

/// 映射新版显式发现流页；它不改变旧版单拍卖 HTTP 合同。
pub(super) fn map_discovery_page(page: PublicRealtimePage) -> PublicRealtimeDiscoveryResponse {
    PublicRealtimeDiscoveryResponse {
        scope: "auctions".to_owned(),
        events: page.events.into_iter().map(map_event).collect(),
        next_after: page.next_after,
        has_more: page.has_more,
        refetch_required: page.refetch_required,
    }
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
