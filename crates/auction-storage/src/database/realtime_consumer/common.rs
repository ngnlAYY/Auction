//! Realtime consumer 的共享校验与错误映射。

pub(super) use crate::database::storage_error;
use auction_application::{ApplicationError, RealtimeConsumerRequest};
use uuid::Uuid;

pub(super) fn validate_cursor_keys(
    consumer_key: &str,
    stream_key: &str,
) -> Result<(), ApplicationError> {
    RealtimeConsumerRequest {
        consumer_key: consumer_key.to_owned(),
        stream_key: stream_key.to_owned(),
        batch_size: 1,
        allow_discovery_stream: true,
    }
    .validate()
    .map_err(storage_error)
}

pub(super) fn validate_event_position(
    event_id: Uuid,
    stream_sequence: i64,
) -> Result<(), ApplicationError> {
    if event_id.is_nil() || stream_sequence <= 0 {
        return Err(storage_error("实时事件标识或序号无效"));
    }
    Ok(())
}

pub(super) fn validate_failure_text(
    failure_code: &str,
    safe_detail: &str,
) -> Result<(), ApplicationError> {
    if !auction_domain::valid_text(failure_code, 128)
        || !auction_domain::valid_text(safe_detail, 2_048)
    {
        return Err(storage_error("实时消费失败分类或安全说明无效"));
    }
    Ok(())
}
