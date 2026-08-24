use auction_application::{RealtimeFailureSnapshot, RealtimeLagSnapshot};
use auction_contracts::{AdminRealtimeFailureResponse, AdminRealtimeLagResponse};

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
