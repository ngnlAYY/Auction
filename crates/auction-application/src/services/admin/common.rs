//! Admin 命令信封的共享前置条件：命令身份、操作者标识、请求标识与时间戳。

use auction_domain::valid_text;
use chrono::{DateTime, Utc};

/// 所有 Admin 命令都必须满足的信封约束：命令 ID 非 nil、操作者标识非空且不超 256 字符、
/// 可选请求 ID 同样受限、`now` 可表示为纳秒时间戳。各命令自己的字段校验仍由调用方保留。
pub(crate) fn valid_command_envelope(
    command_id: uuid::Uuid,
    actor_id: &str,
    request_id: Option<&str>,
    now: DateTime<Utc>,
) -> bool {
    !command_id.is_nil()
        && valid_text(actor_id, 256)
        && request_id.is_none_or(|value| valid_text(value, 256))
        && now.timestamp_nanos_opt().is_some()
}
