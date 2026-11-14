//! 查询字符串、页码偏移与路径 UUID 的严格解析；失败不携带原因（`Err(())` 或 `None`），由
//! handler 映射为各自路由的稳定错误外壳。

use std::collections::BTreeMap;

use axum::http::Uri;
use url::form_urlencoded;
use uuid::Uuid;

/// 按路由闭集解析查询字符串：出现未登记的键或重复键都失败关闭。
pub(crate) fn parse_query<'a>(
    uri: &Uri,
    allowed: &[&'a str],
) -> Result<BTreeMap<&'a str, String>, ()> {
    let mut values = BTreeMap::new();
    let Some(query) = uri.query() else {
        return Ok(values);
    };
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        // 键只可能来自路由闭集，直接借用允许项；解码及重复键判断仍按原文进行。
        let Some(&key) = allowed.iter().find(|allowed_key| **allowed_key == key) else {
            return Err(());
        };
        if values.insert(key, value.into_owned()).is_some() {
            return Err(());
        }
    }
    Ok(values)
}

/// 解析正整数参数：缺省取 `default`，空串、非数字、非正数或超过 `maximum` 均拒绝。
pub(crate) fn parse_positive_i64(
    value: Option<&String>,
    default: i64,
    maximum: Option<i64>,
) -> Result<i64, ()> {
    let Some(value) = value else {
        return Ok(default);
    };
    if value.is_empty() {
        return Err(());
    }
    let parsed = value.parse::<i64>().map_err(|_| ())?;
    if parsed < 1 || maximum.is_some_and(|maximum| parsed > maximum) {
        return Err(());
    }
    Ok(parsed)
}

/// 把从 1 开始的页码换算成行偏移；乘法溢出或超过 `max_offset` 时返回 `None`，
/// `max_offset` 为 `None` 表示该列表不设偏移上限。页码下界由 [`parse_positive_i64`] 保证。
pub(crate) fn page_offset(page: i64, page_size: i64, max_offset: Option<i64>) -> Option<i64> {
    page.checked_sub(1)
        .and_then(|page| page.checked_mul(page_size))
        .filter(|offset| max_offset.is_none_or(|maximum| *offset <= maximum))
}

/// 解析旧 Public v1 使用的规范小写、非零 UUID。
pub(crate) fn parse_public_uuid(value: &str) -> Result<Uuid, ()> {
    parse_canonical_uuid(value).ok_or(())
}

/// Public 与严格 Admin 路由共用规范 UUID 校验；格式化只使用栈缓冲。
pub(crate) fn parse_canonical_uuid(value: &str) -> Option<Uuid> {
    let id = Uuid::parse_str(value).ok()?;
    if id.is_nil() {
        return None;
    }
    let mut buffer = Uuid::encode_buffer();
    let canonical: &str = id.hyphenated().encode_lower(&mut buffer);
    (canonical == value).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::page_offset;

    #[test]
    fn page_offset_starts_at_zero_for_first_page() {
        assert_eq!(page_offset(1, 50, Some(100)), Some(0));
        assert_eq!(page_offset(1, 50, None), Some(0));
        assert_eq!(page_offset(3, 20, None), Some(40));
    }

    #[test]
    fn page_offset_accepts_the_maximum_and_rejects_beyond_it() {
        assert_eq!(page_offset(3, 50, Some(100)), Some(100));
        assert_eq!(page_offset(4, 50, Some(100)), None);
    }

    #[test]
    fn page_offset_rejects_multiplication_overflow_even_without_maximum() {
        assert_eq!(page_offset(i64::MAX, 2, None), None);
        assert_eq!(page_offset(i64::MAX / 2 + 2, 2, Some(i64::MAX)), None);
    }

    #[test]
    fn page_offset_without_maximum_keeps_large_offsets() {
        assert_eq!(page_offset(i64::MAX / 2, 2, None), Some(i64::MAX - 3));
    }
}
