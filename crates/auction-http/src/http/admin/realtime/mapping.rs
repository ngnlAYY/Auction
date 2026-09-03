use auction_application::{RealtimeEventSnapshot, RealtimeFailureSnapshot, RealtimeLagSnapshot};
use auction_contracts::{
    AdminRealtimeEventResponse, AdminRealtimeFailureResponse, AdminRealtimeLagResponse,
};

/// 将已持久化事件映射为不含 payload 和内部来源字段的 Admin 投影。
pub(crate) fn map_event(event: RealtimeEventSnapshot) -> AdminRealtimeEventResponse {
    AdminRealtimeEventResponse {
        event_id: event.event_id.to_string(),
        stream_key: event.stream_key,
        stream_sequence: event.stream_sequence,
        kind: event.kind,
        schema_version: event.schema_version,
        created_at: super::timestamp(event.created_at),
    }
}

/// 将 realtime lag 映射为安全 HTTP 投影。
pub(crate) fn map_lag(lag: RealtimeLagSnapshot) -> AdminRealtimeLagResponse {
    AdminRealtimeLagResponse {
        stream_key: lag.stream_key,
        consumer_key: lag.consumer_key,
        offset: lag.offset,
        latest_sequence: lag.latest_sequence,
        lag: lag.lag,
    }
}

/// 将 realtime failure 映射为安全 HTTP 投影。
pub(crate) fn map_failure(failure: RealtimeFailureSnapshot) -> AdminRealtimeFailureResponse {
    let resolved = failure.resolved();
    AdminRealtimeFailureResponse {
        id: failure.id.to_string(),
        event_id: failure.event_id.to_string(),
        stream_key: failure.stream_key,
        consumer_key: failure.consumer_key,
        code: failure.code,
        safe_detail: failure.safe_detail,
        failed_at: super::timestamp(failure.failed_at),
        resolved_at: failure.resolved_at.map(super::timestamp),
        resolved,
    }
}
