//! 装备页面解析共用的选择器、文本和字段边界。

use scraper::Selector;

use super::EquipmentParseError;

pub(super) fn selector(raw: &str) -> Result<Selector, EquipmentParseError> {
    Selector::parse(raw).map_err(|_| EquipmentParseError::PageStructure)
}

pub(super) fn normalize_text(value: impl AsRef<str>) -> String {
    let value = value.as_ref().trim();
    let mut normalized = String::with_capacity(value.len());
    for word in value.split_whitespace() {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(word);
    }
    normalized
}

pub(super) fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}
