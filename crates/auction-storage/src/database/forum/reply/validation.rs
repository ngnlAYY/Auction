//! 论坛回帖存储的输入闭集、文本边界和能力错误。

use auction_application::{ApplicationError, ForumBidCommandRecord, RepositoryError};
#[cfg(feature = "postgres")]
use auction_domain::SettlementWorld;

pub(super) const MAX_REPLY_BATCH_SIZE: u16 = 100;
pub(super) const MAX_DISPLAY_NAME_BYTES: usize = 256;
pub(super) const MAX_FORUM_PLAYER_ID_BYTES: usize = 32;
pub(super) const MAX_THREAD_ID_BYTES: usize = 64;
pub(super) const MAX_FIXED_POST_ID_BYTES: usize = 32;
pub(super) const MAX_POST_ID_BYTES: usize = 256;
pub(super) const MAX_LOT_CODE_BYTES: usize = 16;
pub(super) const MAX_ACTOR_BYTES: usize = 256;
pub(super) const MAX_COMMAND_KIND_BYTES: usize = 32;
pub(super) const MAX_ERROR_CODE_BYTES: usize = 128;
pub(super) const MAX_ERROR_DETAIL_BYTES: usize = 2048;

pub(super) fn valid_command_record(record: &ForumBidCommandRecord) -> bool {
    !record.command_id.is_nil()
        && !record.auction_id.is_nil()
        && valid_text(&record.thread_id, MAX_THREAD_ID_BYTES)
        && valid_text(&record.post_id, MAX_POST_ID_BYTES)
        && record.line_number > 0
        && valid_text(&record.lot_code, MAX_LOT_CODE_BYTES)
        && valid_text(&record.actor, MAX_ACTOR_BYTES)
        && matches!(record.command_kind.as_str(), "bid" | "proxy" | "proxy_lower" | "notify")
        && record.command_kind.len() <= MAX_COMMAND_KIND_BYTES
        && record.amount >= 0
}

pub(super) fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

pub(super) fn valid_optional_text(value: &str, maximum: usize) -> bool {
    value.is_empty() || valid_text(value, maximum)
}

pub(super) fn valid_decimal_id(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.as_bytes()[0].is_ascii_digit()
        && value.as_bytes()[0] != b'0'
        && value.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(feature = "postgres")]
pub(super) fn parse_world(value: &str) -> Result<SettlementWorld, ApplicationError> {
    super::super::sync::parse_world(value)
}

pub(super) fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}

#[cfg(feature = "sqlite")]
pub(super) fn capability_error() -> ApplicationError {
    ApplicationError::CapabilityUnavailable("论坛回帖导入仅支持 PostgreSQL 完整档")
}
