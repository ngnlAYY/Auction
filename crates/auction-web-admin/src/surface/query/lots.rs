//! lot 列表的 query 状态：页码与排序。

use auction_web_shared::{QueryState, use_query_state};

use super::super::sort::SortDirection;
use super::primitives::{
    parse_page, parse_sort_column, parse_sort_direction, push_page, push_sort,
};

const LOTS_KEYS: &[&str] = &["page", "sort", "dir"];

/// lot 列表可排序列：序号、状态、起拍价、当前价。
const LOTS_SORT_COLUMNS: &[&str] = &["code", "status", "start_price", "current_price"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LotsQuery {
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序。合法值见 `LOTS_SORT_COLUMNS`。
    pub sort: String,
    pub direction: SortDirection,
}

impl Default for LotsQuery {
    fn default() -> Self {
        Self { page: 1, sort: String::new(), direction: SortDirection::Asc }
    }
}

pub(crate) fn use_lots_query_state() -> QueryState<LotsQuery> {
    use_query_state(
        LOTS_KEYS,
        |pairs| LotsQuery {
            page: parse_page(pairs, "page"),
            sort: parse_sort_column(pairs, LOTS_SORT_COLUMNS),
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
