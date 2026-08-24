//! 装备页面解析共用的选择器、文本和字段边界。

use auction_application::AdminIntakeError;
use scraper::Selector;

pub(super) fn selector(raw: &str) -> Result<Selector, AdminIntakeError> {
    Selector::parse(raw).map_err(|_| AdminIntakeError::InvalidSource)
}

pub(super) fn normalize_text(value: impl AsRef<str>) -> String {
    value.as_ref().split_whitespace().collect::<Vec<_>>().join(" ")
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
