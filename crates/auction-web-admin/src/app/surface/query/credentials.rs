//! 只含页码与排序的列表页 query 状态（凭据列表）。

use auction_web_shared::{QueryState, use_query_state};

use super::super::sort::SortDirection;
use super::primitives::{
    parse_page, parse_sort_column, parse_sort_direction, push_page, push_sort,
};

const PAGE_ONLY_KEYS: &[&str] = &["page", "sort", "dir"];

/// 凭据列表可排序列：标签、世界、类型、状态、上次检查时间。
const CREDENTIALS_SORT_COLUMNS: &[&str] = &["label", "world", "kind", "status", "checked"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PageQuery {
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序。合法值见 `CREDENTIALS_SORT_COLUMNS`。
    pub sort: String,
    pub direction: SortDirection,
}

impl Default for PageQuery {
    fn default() -> Self {
        Self { page: 1, sort: String::new(), direction: SortDirection::Asc }
    }
}

pub(crate) fn use_page_query_state() -> QueryState<PageQuery> {
    use_query_state(
        PAGE_ONLY_KEYS,
        |pairs| PageQuery {
            page: parse_page(pairs, "page"),
            sort: parse_sort_column(pairs, CREDENTIALS_SORT_COLUMNS),
            direction: parse_sort_direction(pairs),
        },
        |query| {
            let mut pairs = Vec::new();
            push_page(&mut pairs, "page", query.page);
            push_sort(&mut pairs, &query.sort, query.direction);
            pairs
        },
    )
}
