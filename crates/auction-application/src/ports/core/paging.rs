//! 有界数字分页的共享窗口校验。

/// `limit` 必须为正且不超过 `max_page_size`，`offset` 必须落在 `0..=max_offset`。
/// 各端口的筛选、游标与身份校验仍由调用方保留，这里只固定分页窗口本身。
pub fn valid_page_window(limit: i64, offset: i64, max_page_size: i64, max_offset: i64) -> bool {
    limit > 0 && limit <= max_page_size && (0..=max_offset).contains(&offset)
}
