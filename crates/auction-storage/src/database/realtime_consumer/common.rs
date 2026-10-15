//! Realtime consumer 的共享校验与错误映射。

use auction_application::{ApplicationError, RealtimeConsumerRequest, RepositoryError};
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
    if failure_code.trim().is_empty()
        || failure_code != failure_code.trim()
        || failure_code.len() > 128
        || failure_code.chars().any(char::is_control)
        || safe_detail.trim().is_empty()
        || safe_detail != safe_detail.trim()
        || safe_detail.len() > 2_048
        || safe_detail.chars().any(char::is_control)
    {
        return Err(storage_error("实时消费失败分类或安全说明无效"));
    }
    Ok(())
}

pub(super) fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
