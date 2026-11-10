//! realtime 视图的 query 状态：事件/失败/延迟三个分页各自独立入 URL，另含自动轮询间隔。

use auction_web_shared::{QueryState, use_query_state};

use super::primitives::{
    parse_choice, parse_page, parse_poll_seconds, parse_text, push_page, push_poll_seconds,
    push_text,
};

const REALTIME_KEYS: &[&str] = &[
    "event_stream",
    "event_kind",
    "event_page",
    "failure_stream",
    "failure_consumer",
    "failure_code",
    "failure_state",
    "failure_page",
    "lag_page",
    "poll",
];

/// failure 状态下拉只开放这两个可选值（pending/retry 选项在页面上被禁用）。
const REALTIME_FAILURE_STATE_VALUES: &[&str] = &["dead_letter", "completed"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RealtimeQuery {
    pub event_stream: String,
    pub event_kind: String,
    pub event_page: i64,
    pub failure_stream: String,
    pub failure_consumer: String,
    pub failure_code: String,
    /// 空串表示全部状态；合法值见 `REALTIME_FAILURE_STATE_VALUES`。
    pub failure_state: String,
    pub failure_page: i64,
    pub lag_page: i64,
    /// 自动轮询间隔秒数；0 表示关闭（默认）。合法值见 `POLL_INTERVAL_VALUES`。
    pub poll_seconds: u64,
}

/// realtime 三个分页各自独立入 URL，默认值都是第一页。
impl Default for RealtimeQuery {
    fn default() -> Self {
        Self {
            event_stream: String::new(),
            event_kind: String::new(),
            event_page: 1,
            failure_stream: String::new(),
            failure_consumer: String::new(),
            failure_code: String::new(),
            failure_state: String::new(),
            failure_page: 1,
            lag_page: 1,
            poll_seconds: 0,
        }
    }
}

pub(crate) fn use_realtime_query_state() -> QueryState<RealtimeQuery> {
    use_query_state(
        REALTIME_KEYS,
        |pairs| RealtimeQuery {
            event_stream: parse_text(pairs, "event_stream", 256),
            event_kind: parse_text(pairs, "event_kind", 128),
            event_page: parse_page(pairs, "event_page"),
            failure_stream: parse_text(pairs, "failure_stream", 256),
            failure_consumer: parse_text(pairs, "failure_consumer", 128),
            failure_code: parse_text(pairs, "failure_code", 64),
            failure_state: parse_choice(pairs, "failure_state", REALTIME_FAILURE_STATE_VALUES),
            failure_page: parse_page(pairs, "failure_page"),
            lag_page: parse_page(pairs, "lag_page"),
            poll_seconds: parse_poll_seconds(pairs),
        },
        |query| {
            let mut pairs = Vec::new();
            push_text(&mut pairs, "event_stream", &query.event_stream);
            push_text(&mut pairs, "event_kind", &query.event_kind);
            push_page(&mut pairs, "event_page", query.event_page);
            push_text(&mut pairs, "failure_stream", &query.failure_stream);
            push_text(&mut pairs, "failure_consumer", &query.failure_consumer);
            push_text(&mut pairs, "failure_code", &query.failure_code);
            push_text(&mut pairs, "failure_state", &query.failure_state);
            push_page(&mut pairs, "failure_page", query.failure_page);
            push_page(&mut pairs, "lag_page", query.lag_page);
            push_poll_seconds(&mut pairs, query.poll_seconds);
            pairs
        },
    )
}
