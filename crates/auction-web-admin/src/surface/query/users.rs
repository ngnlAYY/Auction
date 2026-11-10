//! 用户列表的 query 状态：用户名、状态筛选、页码与排序。名称输入框是提交前草稿，不入 URL。

use auction_web_shared::{QueryState, use_query_state};

use crate::api::AdminUserStatusFilter;

use super::super::sort::SortDirection;
use super::primitives::{
    pair_value, parse_page, parse_sort_column, parse_sort_direction, parse_text, push_page,
    push_sort, push_text,
};

const USERS_KEYS: &[&str] = &["name", "status", "page", "sort", "dir"];

/// 用户列表可排序列：用户名、状态、拉黑次数、创建时间。
const USERS_SORT_COLUMNS: &[&str] = &["username", "status", "blacklist_count", "created_at"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UsersQuery {
    pub name: String,
    pub status: AdminUserStatusFilter,
    pub page: i64,
    /// 排序列标识；空串表示保持服务端原始顺序。合法值见 `USERS_SORT_COLUMNS`。
    pub sort: String,
    pub direction: SortDirection,
}

impl Default for UsersQuery {
    fn default() -> Self {
        Self {
            name: String::new(),
            status: AdminUserStatusFilter::All,
            page: 1,
            sort: String::new(),
            direction: SortDirection::Asc,
        }
    }
}

/// 状态筛选序列化：`All` 是默认值，不写地址栏。
fn users_status_value(status: AdminUserStatusFilter) -> Option<&'static str> {
    match status {
        AdminUserStatusFilter::All => None,
        AdminUserStatusFilter::Active => Some("active"),
        AdminUserStatusFilter::Blacklisted => Some("blacklisted"),
    }
}

pub(crate) fn use_users_query_state() -> QueryState<UsersQuery> {
    use_query_state(
        USERS_KEYS,
        |pairs| UsersQuery {
            name: parse_text(pairs, "name", 256),
            status: match pair_value(pairs, "status") {
                Some("active") => AdminUserStatusFilter::Active,
                Some("blacklisted") => AdminUserStatusFilter::Blacklisted,
                // 非法值与缺省都回退「全部」，与页面下拉的默认选项一致。
                _ => AdminUserStatusFilter::All,
            },
            page: parse_page(pairs, "page"),
            sort: parse_sort_column(pairs, USERS_SORT_COLUMNS),
            direction: parse_sort_direction(pairs),
        },
        |query| {
            let mut pairs = Vec::new();
            push_text(&mut pairs, "name", &query.name);
            if let Some(status) = users_status_value(query.status) {
                pairs.push(("status".to_owned(), status.to_owned()));
            }
            push_page(&mut pairs, "page", query.page);
            push_sort(&mut pairs, &query.sort, query.direction);
            pairs
        },
    )
}
