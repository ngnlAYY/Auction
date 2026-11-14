//! 论坛快照字段的闭集校验：编号、文本、链接与当前出价来源不合规时拒绝渲染。

use auction_application::forum::{ForumCurrentBidSource, ForumLotSnapshot};
use auction_domain::valid_text;
use url::Url;

pub(super) fn valid_forum_post_id(value: &str) -> bool {
    auction_domain::valid_positive_decimal(value, 32)
}

pub(super) fn valid_lot(lot: &ForumLotSnapshot) -> bool {
    valid_code(&lot.code)
        && valid_text(&lot.category, 128)
        && valid_text(&lot.name, 1024)
        && valid_optional_text(&lot.equipment_quality, 64)
        && valid_optional_text(&lot.equipment_type, 64)
        && valid_optional_text(&lot.equipment_prefix, 128)
        && valid_optional_text(&lot.equipment_suffix, 128)
        && lot.equipment_level.is_none_or(|level| (1..=100_000).contains(&level))
        && lot.primary_attribute_initials.as_deref().is_none_or(valid_primary_attribute_initials)
        && lot.avg_roll_tenths.is_none_or(|value| (0..=1_000).contains(&value))
        && valid_text(&lot.seller, 256)
        && lot.start_price > 0
        && lot.current_price >= lot.start_price
        && valid_text(&lot.status, 64)
        && lot.current_bidder.as_deref().is_none_or(|value| valid_text(value, 256))
        && lot.current_bid_source.as_ref().is_none_or(valid_current_bid_source)
        && (lot.current_bid_source.is_none() || lot.current_bidder.is_some())
        && lot.effective_end_at.is_none_or(|value| value.timestamp_nanos_opt().is_some())
        && lot.equipment_link_target.validate().is_ok()
}

pub(super) fn valid_current_bid_source(source: &ForumCurrentBidSource) -> bool {
    match source {
        ForumCurrentBidSource::Forum { post_number, thread_id, post_id } => {
            *post_number >= 3 && valid_forum_post_id(thread_id) && valid_forum_post_id(post_id)
        }
        ForumCurrentBidSource::Proxy => true,
    }
}

pub(super) fn valid_code(value: &str) -> bool {
    let bytes = value.as_bytes();
    (5..=13).contains(&bytes.len())
        && bytes[..3].iter().all(u8::is_ascii_alphabetic)
        && bytes[3..].iter().all(u8::is_ascii_digit)
        && value[3..].parse::<i32>().is_ok_and(|number| number > 0)
}

pub(super) fn valid_primary_attribute_initials(value: &str) -> bool {
    !value.is_empty() && value.len() <= 6 && value.bytes().all(|byte| byte.is_ascii_uppercase())
}

pub(super) fn valid_optional_text(value: &str, maximum: usize) -> bool {
    value.is_empty() || valid_text(value, maximum)
}

pub(super) fn valid_public_url(value: &str) -> bool {
    if !valid_text(value, 2048) {
        return false;
    }
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && !value.contains(['[', ']'])
}
