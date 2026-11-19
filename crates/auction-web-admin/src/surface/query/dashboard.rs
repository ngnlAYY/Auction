//! 仪表盘的 query 状态：快照本身是资源瞬态，只有轮询开关入 URL。

use auction_web_shared::{QueryState, use_query_state};

#[cfg(test)]
use super::super::realtime::use_test_history;
use super::primitives::parse_poll_seconds_with_default;

const DASHBOARD_KEYS: &[&str] = &["poll"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DashboardQuery {
    pub poll_seconds: u64,
}

pub(crate) fn use_dashboard_query_state() -> QueryState<DashboardQuery> {
    let query = use_query_state(DASHBOARD_KEYS, parse_query, query_pairs);
    #[cfg(test)]
    use_test_history(query.state, parse_query, query_pairs);
    query
}

fn parse_query(pairs: &[(String, String)]) -> DashboardQuery {
    // 显式 `poll=0` 才关闭默认轮询。
    DashboardQuery { poll_seconds: parse_poll_seconds_with_default(pairs, 30) }
}

fn query_pairs(query: &DashboardQuery) -> Vec<(String, String)> {
    // 也保存 0，避免刷新把关闭状态解析回默认开启。
    vec![("poll".to_owned(), query.poll_seconds.to_string())]
}
