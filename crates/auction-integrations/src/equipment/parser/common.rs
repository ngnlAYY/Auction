//! 装备页面解析共用的文本和字段边界。
pub(super) use auction_domain::valid_text;

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
