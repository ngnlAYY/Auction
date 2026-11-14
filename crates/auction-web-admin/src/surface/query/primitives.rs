//! 各列表页共用的 query 原语：取值、页码、闭集枚举、自由文本、排序、轮询间隔与 UUID 校验。
//! 校验失败的地址栏值一律回退到「未设置」，不把畸形文本转发给 API。

use super::super::poll::POLL_INTERVAL_VALUES;
use super::super::sort::SortDirection;

/// 取某个键的首个值。
pub(super) fn pair_value<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs.iter().find(|(pair_key, _)| pair_key == key).map(|(_, value)| value.as_str())
}

/// 解析正整数页码；缺失、零、负数或非法字符串都回退第一页。
pub(super) fn parse_page(pairs: &[(String, String)], key: &str) -> i64 {
    pair_value(pairs, key)
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|page| *page > 0)
        .unwrap_or(1)
}

/// 页码大于 1 时才写入地址栏。
pub(super) fn push_page(pairs: &mut Vec<(String, String)>, key: &str, page: i64) {
    if page > 1 {
        pairs.push((key.to_owned(), page.to_string()));
    }
}

/// 解析闭集枚举值；不在允许集合内（含缺省）时回退空串表示「全部」。
pub(super) fn parse_choice(pairs: &[(String, String)], key: &str, allowed: &[&str]) -> String {
    pair_value(pairs, key).filter(|value| allowed.contains(value)).unwrap_or_default().to_owned()
}

/// 解析自由文本筛选：trim、长度上限（与对应输入框 maxlength 对齐）、剔除控制字符；
/// 不满足任一条件视为未设置。
pub(super) fn parse_text(pairs: &[(String, String)], key: &str, maximum: usize) -> String {
    let Some(raw) = pair_value(pairs, key) else {
        return String::new();
    };
    let value = raw.trim();
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        String::new()
    } else {
        value.to_owned()
    }
}

/// 文本筛选非空时才写入地址栏。
pub(super) fn push_text(pairs: &mut Vec<(String, String)>, key: &str, value: &str) {
    if !value.is_empty() {
        pairs.push((key.to_owned(), value.to_owned()));
    }
}

/// 解析排序列：不在该页允许集合内（含缺省）回退空串，表示保持服务端原始顺序。
pub(super) fn parse_sort_column(pairs: &[(String, String)], allowed: &[&str]) -> String {
    parse_choice(pairs, "sort", allowed)
}

/// 解析排序方向；只有排序列存在时方向才有意义，非法值回退升序。
pub(super) fn parse_sort_direction(pairs: &[(String, String)]) -> SortDirection {
    SortDirection::parse(pair_value(pairs, "dir"))
}

/// 解析自动轮询间隔：合法值为 0（关闭）与 10/30/60 秒；缺省回退调用方默认值。
pub(super) fn parse_poll_seconds_with_default(pairs: &[(String, String)], default: u64) -> u64 {
    pair_value(pairs, "poll")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds == 0 || POLL_INTERVAL_VALUES.contains(seconds))
        .unwrap_or(default)
}

/// 一般列表页默认关闭自动轮询。
pub(super) fn parse_poll_seconds(pairs: &[(String, String)]) -> u64 {
    parse_poll_seconds_with_default(pairs, 0)
}

/// 轮询开启（间隔非 0）时才写入地址栏。
pub(super) fn push_poll_seconds(pairs: &mut Vec<(String, String)>, seconds: u64) {
    if seconds > 0 {
        pairs.push(("poll".to_owned(), seconds.to_string()));
    }
}

/// 排序列非空时才写入地址栏；方向仅降序（非默认）时写入 `dir=desc`。
pub(super) fn push_sort(pairs: &mut Vec<(String, String)>, sort: &str, direction: SortDirection) {
    if sort.is_empty() {
        return;
    }
    pairs.push(("sort".to_owned(), sort.to_owned()));
    if let Some(direction) = direction.as_query_value() {
        pairs.push(("dir".to_owned(), direction.to_owned()));
    }
}

/// 只接受规范 UUID；长度与字符集不符时按未设置处理，避免用地址栏文本扫描。
pub(super) fn parse_uuid_text(pairs: &[(String, String)], key: &str) -> String {
    let value = parse_text(pairs, key, 36);
    if crate::api::valid_uuid(&value) { value } else { String::new() }
}
