//! 浏览器请求与响应分页边界；只校验显式参数，不读取展示偏好或截断用户请求。

use super::MAX_JSON_SAFE_INTEGER;

pub(super) fn validate(page: i64, size: i64, maximum: i64, max_offset: i64) -> Result<(), String> {
    if page <= 0
        || size <= 0
        || size > maximum
        || page
            .checked_sub(1)
            .and_then(|value| value.checked_mul(size))
            .is_none_or(|offset| offset > max_offset)
    {
        return Err("admin_pagination_bounds".to_owned());
    }
    Ok(())
}

/// 响应分页三元组均须是 JSON 安全整数：页码与页大小为正，总数非负。
pub(super) fn valid_response(page: i64, page_size: i64, total: i64) -> bool {
    (1..=MAX_JSON_SAFE_INTEGER).contains(&page)
        && (1..=MAX_JSON_SAFE_INTEGER).contains(&page_size)
        && (0..=MAX_JSON_SAFE_INTEGER).contains(&total)
}
