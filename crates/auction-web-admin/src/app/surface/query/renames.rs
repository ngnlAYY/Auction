//! 改名历史的 query 状态：用户名筛选、页码与排序。名称输入框是提交前草稿，不入 URL。

use auction_web_shared::{QueryState, use_query_state};

use super::super::sort::SortDirection;
use super::primitives::{
    parse_page, parse_sort_column, parse_sort_direction, parse_text, push_page, push_sort,
    push_text,
};

const RENAMES_KEYS: &[&str] = &["name", "page", "sort", "dir"];

/// 改名历史可排序列：改名时间、原用户名、新用户名、操作者。
const RENAMES_SORT_COLUMNS: &[&str] = &["created", "previous", "next", "actor"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenamesQuery {
    pub name: String,
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序。合法值见 `RENAMES_SORT_COLUMNS`。
    pub sort: String,
    pub direction: SortDirection,
}

impl Default for RenamesQuery {
    fn default() -> Self {
        Self { name: String::new(), page: 1, sort: String::new(), direction: SortDirection::Asc }
    }
}

pub(crate) fn use_renames_query_state() -> QueryState<RenamesQuery> {
    use_query_state(
        RENAMES_KEYS,
        |pairs| RenamesQuery {
            name: parse_text(pairs, "name", 256),
            page: parse_page(pairs, "page"),
            sort: parse_sort_column(pairs, RENAMES_SORT_COLUMNS),
            direction: parse_sort_direction(pairs),
        },
        |query| {
            let mut pairs = Vec::new();
            push_text(&mut pairs, "name", &query.name);
            push_page(&mut pairs, "page", query.page);
            push_sort(&mut pairs, &query.sort, query.direction);
            pairs
        },
    )
}
