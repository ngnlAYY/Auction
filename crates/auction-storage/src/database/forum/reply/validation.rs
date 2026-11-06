//! 论坛回帖存储的输入闭集、文本边界和能力错误。

pub(super) use crate::database::storage_error;
use auction_application::core::ApplicationError;
use auction_application::forum::ForumBidCommandRecord;
#[cfg(feature = "postgres")]
use auction_domain::SettlementWorld;
pub(super) use auction_domain::valid_text;

pub(super) const MAX_REPLY_BATCH_SIZE: u16 = 100;
pub(super) const MAX_PLAYER_NAME_BYTES: usize = 256;
pub(super) const MAX_FORUM_ACCOUNT_ID_BYTES: usize = 32;
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
        && record.post_number >= 3
        && record.line_number > 0
        && valid_text(&record.lot_code, MAX_LOT_CODE_BYTES)
        && valid_text(&record.actor, MAX_ACTOR_BYTES)
        && matches!(record.command_kind.as_str(), "bid" | "proxy" | "proxy_lower" | "notify")
        && record.command_kind.len() <= MAX_COMMAND_KIND_BYTES
        && record.amount >= 0
}

pub(super) fn valid_optional_text(value: &str, maximum: usize) -> bool {
    value.is_empty() || valid_text(value, maximum)
}

pub(super) use super::super::sync::validation::valid_decimal_id;

#[cfg(feature = "postgres")]
pub(super) fn parse_world(value: &str) -> Result<SettlementWorld, ApplicationError> {
    super::super::sync::parse_world(value)
}
