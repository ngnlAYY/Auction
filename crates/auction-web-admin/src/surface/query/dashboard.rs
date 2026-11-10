//! 仪表盘的 query 状态：快照本身是资源瞬态，只有轮询开关入 URL。

use auction_web_shared::{QueryState, use_query_state};

use super::primitives::parse_poll_seconds_with_default;

const DASHBOARD_KEYS: &[&str] = &["poll"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DashboardQuery {
    pub poll_seconds: u64,
}

pub(crate) fn use_dashboard_query_state() -> QueryState<DashboardQuery> {
    use_query_state(
        DASHBOARD_KEYS,
        |pairs| DashboardQuery {
            // 仪表盘默认开启 30 秒自动轮询；显式 `poll=0` 才关闭。
            poll_seconds: parse_poll_seconds_with_default(pairs, 30),
        },
        |query| {
            // 仪表盘默认值也是 30 秒，因此必须显式写入当前值（含 0），否则关闭后刷新
            // 会被解析回默认开启。
            vec![("poll".to_owned(), query.poll_seconds.to_string())]
        },
    )
}
