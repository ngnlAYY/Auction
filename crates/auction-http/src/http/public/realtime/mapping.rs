//! Public 实时事件页校验与安全 DTO 映射。

use auction_application::{PublicRealtimeEvent, PublicRealtimePage};
use auction_contracts::{PublicRealtimeEventResponse, PublicRealtimeResponse};
use uuid::Uuid;

use super::super::timestamp;

pub(super) fn valid_page(page: &PublicRealtimePage, auction_id: Option<Uuid>, after: i64) -> bool {
    let expected_stream = auction_id.map_or_else(
        || "auction:discovery".to_owned(),
        |auction_id| format!("auction:{auction_id}"),
    );
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
    page.next_after == previous
}

pub(super) fn map_page(page: PublicRealtimePage) -> PublicRealtimeResponse {
    PublicRealtimeResponse {
        events: page.events.into_iter().map(map_event).collect(),
        next_after: page.next_after,
        has_more: page.has_more,
        refetch_required: page.refetch_required,
    }
}

fn map_event(event: PublicRealtimeEvent) -> PublicRealtimeEventResponse {
    PublicRealtimeEventResponse {
        event_id: event.event_id.to_string(),
        stream_key: event.stream_key,
        stream_sequence: event.stream_sequence,
        kind: event.kind,
        schema_version: event.schema_version,
        payload: event.payload,
        created_at: timestamp(event.created_at),
    }
}
