//! 浏览器请求分页边界；只校验显式参数，不读取展示偏好或截断用户请求。

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
