//! 装备页面解析共用的文本规范化。

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
