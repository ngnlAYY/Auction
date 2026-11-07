//! 拍卖列表（场次）的 query 状态：世界筛选、页码与排序。

use auction_web_shared::{QueryState, use_query_state};

use super::super::sort::SortDirection;
use super::primitives::{
    parse_choice, parse_page, parse_sort_column, parse_sort_direction, push_page, push_sort,
    push_text,
};

const AUCTIONS_KEYS: &[&str] = &["world", "page", "sort", "dir"];

/// 拍卖列表可排序列：场次号、状态、开拍时间、结拍时间。
const AUCTIONS_SORT_COLUMNS: &[&str] = &["sequence", "status", "open_at", "end_at"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuctionsQuery {
    /// 空串表示全部世界；合法值为 `main` / `isekai`。
    pub world: String,
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序。合法值见 `AUCTIONS_SORT_COLUMNS`。
    pub sort: String,
    /// 排序方向；仅 `sort` 非空时有意义。
    pub direction: SortDirection,
}

impl Default for AuctionsQuery {
    fn default() -> Self {
        Self { world: String::new(), page: 1, sort: String::new(), direction: SortDirection::Asc }
    }
}

pub(crate) fn use_auctions_query_state() -> QueryState<AuctionsQuery> {
    use_query_state(
        AUCTIONS_KEYS,
        |pairs| AuctionsQuery {
            world: parse_choice(pairs, "world", &["main", "isekai"]),
            page: parse_page(pairs, "page"),
            sort: parse_sort_column(pairs, AUCTIONS_SORT_COLUMNS),
            direction: parse_sort_direction(pairs),
        },
        |query| {
            let mut pairs = Vec::new();
            push_text(&mut pairs, "world", &query.world);
            push_page(&mut pairs, "page", query.page);
            push_sort(&mut pairs, &query.sort, query.direction);
            pairs
        },
    )
}
