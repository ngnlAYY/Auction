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

/// 页码分页响应的回显校验：三元组本身合法，页码与页大小原样回显请求，本页条数不超过页大小
/// 与总数。页大小上限、分类计数、游标与筛选回显等端点特有约束由调用方另行校验。
pub(super) fn valid_page_echo(
    requested_page: i64,
    requested_size: i64,
    page: i64,
    page_size: i64,
    total: i64,
    item_count: usize,
) -> bool {
    valid_response(page, page_size, total)
        && page == requested_page
        && page_size == requested_size
        && i64::try_from(item_count).is_ok_and(|count| count <= page_size && count <= total)
}

#[cfg(test)]
mod tests {
    use super::{MAX_JSON_SAFE_INTEGER, valid_page_echo};

    #[test]
    fn page_echo_accepts_matching_bounded_pages() {
        assert!(valid_page_echo(1, 20, 1, 20, 0, 0));
        assert!(valid_page_echo(3, 20, 3, 20, 45, 5));
        // 页码越过末页时服务端仍回显请求页，条数为 0。
        assert!(valid_page_echo(9, 20, 9, 20, 45, 0));
    }

    #[test]
    fn page_echo_rejects_mismatch_and_overflowing_counts() {
        // 页码或页大小未原样回显。
        assert!(!valid_page_echo(2, 20, 1, 20, 45, 20));
        assert!(!valid_page_echo(1, 20, 1, 50, 45, 20));
        // 条数超过页大小或总数。
        assert!(!valid_page_echo(1, 20, 1, 20, 45, 21));
        assert!(!valid_page_echo(1, 20, 1, 20, 3, 4));
        // 三元组本身越界：页码/页大小非正、总数为负或越过 JSON 安全整数。
        assert!(!valid_page_echo(0, 20, 0, 20, 0, 0));
        assert!(!valid_page_echo(1, 0, 1, 0, 0, 0));
        assert!(!valid_page_echo(1, 20, 1, 20, -1, 0));
        let unsafe_total = MAX_JSON_SAFE_INTEGER + 1;
        assert!(!valid_page_echo(1, 20, 1, 20, unsafe_total, 0));
    }
}
