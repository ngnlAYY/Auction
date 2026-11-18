//! realtime 视图的 query 状态：事件/失败/延迟三个分页各自独立入 URL，另含自动轮询间隔。

use auction_web_shared::{QueryState, use_query_state};
use dioxus::prelude::*;

#[cfg(test)]
use super::super::realtime::use_test_history;
use super::super::realtime::{FilterField, FilterValue};
use super::primitives::{parse_page, parse_poll_seconds, push_page, push_poll_seconds};

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

type EventPageRequest = (i64, String, String);
type FailurePageRequest = (i64, String, String, String, String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RealtimeQuery {
    pub event_stream: FilterValue,
    pub event_kind: FilterValue,
    pub event_page: i64,
    pub failure_stream: FilterValue,
    pub failure_consumer: FilterValue,
    pub failure_code: FilterValue,
    /// 空串表示全部状态，非法历史值保留以便更正。
    pub failure_state: FilterValue,
    pub failure_page: i64,
    pub lag_page: i64,
    /// 自动轮询间隔秒数；0 表示关闭（默认）。合法值见 `POLL_INTERVAL_VALUES`。
    pub poll_seconds: u64,
}

/// realtime 三个分页各自独立入 URL，默认值都是第一页。
impl Default for RealtimeQuery {
    fn default() -> Self {
        Self {
            event_stream: FilterValue::default(),
            event_kind: FilterValue::default(),
            event_page: 1,
            failure_stream: FilterValue::default(),
            failure_consumer: FilterValue::default(),
            failure_code: FilterValue::default(),
            failure_state: FilterValue::default(),
            failure_page: 1,
            lag_page: 1,
            poll_seconds: 0,
        }
    }
}

impl RealtimeQuery {
    /// 非法历史筛选保留原值，但不能派生出可发送的请求。
    pub(crate) fn event_page_request(&self) -> Option<EventPageRequest> {
        Some((
            self.event_page,
            FilterField::EventStream.normalize(self.event_stream.text()?)?.to_owned(),
            FilterField::EventKind.normalize(self.event_kind.text()?)?.to_owned(),
        ))
    }

    /// 失败列表需要全部已应用条件合法，避免以更宽条件读取记录。
    pub(crate) fn failure_page_request(&self) -> Option<FailurePageRequest> {
        Some((
            self.failure_page,
            FilterField::FailureStream.normalize(self.failure_stream.text()?)?.to_owned(),
            FilterField::FailureConsumer.normalize(self.failure_consumer.text()?)?.to_owned(),
            FilterField::FailureCode.normalize(self.failure_code.text()?)?.to_owned(),
            FilterField::FailureState.normalize(self.failure_state.text()?)?.to_owned(),
        ))
    }

    /// 应用事件筛选并返回事件列表的第一页。
    pub(crate) fn with_event_filters(mut self, (stream, kind): (String, String)) -> Self {
        self.event_stream = stream.into();
        self.event_kind = kind.into();
        self.event_page = 1;
        self
    }

    /// 应用失败文本筛选并返回失败列表的第一页。
    pub(crate) fn with_failure_filters(
        mut self,
        (stream, consumer, code): (String, String, String),
    ) -> Self {
        self.failure_stream = stream.into();
        self.failure_consumer = consumer.into();
        self.failure_code = code.into();
        self.failure_page = 1;
        self
    }

    #[cfg(test)]
    pub(crate) fn address_pairs(&self) -> Vec<(String, String)> {
        realtime_query_pairs(self)
    }
}

/// 地址栏状态与两个表单实际订阅的筛选。
pub(crate) struct RealtimeQueryState {
    pub location: QueryState<RealtimeQuery>,
    pub event_filters: Memo<(FilterValue, FilterValue)>,
    pub failure_filters: Memo<(FilterValue, FilterValue, FilterValue, FilterValue)>,
}

pub(crate) fn use_realtime_query_state() -> RealtimeQueryState {
    let location = use_query_state(REALTIME_KEYS, parse_realtime_query, realtime_query_pairs);
    let state = location.state;
    #[cfg(test)]
    use_test_history(state, parse_realtime_query, realtime_query_pairs);
    // 翻页和轮询设置会更新地址栏，但表单只跟随各自已应用的筛选。
    let event_filters = use_memo(move || {
        let query = state.read();
        (query.event_stream.clone(), query.event_kind.clone())
    });
    let failure_filters = use_memo(move || {
        let query = state.read();
        (
            query.failure_stream.clone(),
            query.failure_consumer.clone(),
            query.failure_code.clone(),
            query.failure_state.clone(),
        )
    });
    RealtimeQueryState { location, event_filters, failure_filters }
}

fn realtime_query_pairs(query: &RealtimeQuery) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    query.event_stream.append_pairs(&mut pairs, "event_stream");
    query.event_kind.append_pairs(&mut pairs, "event_kind");
    push_page(&mut pairs, "event_page", query.event_page);
    query.failure_stream.append_pairs(&mut pairs, "failure_stream");
    query.failure_consumer.append_pairs(&mut pairs, "failure_consumer");
    query.failure_code.append_pairs(&mut pairs, "failure_code");
    query.failure_state.append_pairs(&mut pairs, "failure_state");
    push_page(&mut pairs, "failure_page", query.failure_page);
    push_page(&mut pairs, "lag_page", query.lag_page);
    push_poll_seconds(&mut pairs, query.poll_seconds);
    pairs
}

fn parse_realtime_query(pairs: &[(String, String)]) -> RealtimeQuery {
    let filter = |key, field: FilterField| {
        FilterValue::restore(pairs, key, |value| field.normalize(value).map(str::to_owned))
    };
    RealtimeQuery {
        event_stream: filter("event_stream", FilterField::EventStream),
        event_kind: filter("event_kind", FilterField::EventKind),
        event_page: parse_page(pairs, "event_page"),
        failure_stream: filter("failure_stream", FilterField::FailureStream),
        failure_consumer: filter("failure_consumer", FilterField::FailureConsumer),
        failure_code: filter("failure_code", FilterField::FailureCode),
        failure_state: filter("failure_state", FilterField::FailureState),
        failure_page: parse_page(pairs, "failure_page"),
        lag_page: parse_page(pairs, "lag_page"),
        poll_seconds: parse_poll_seconds(pairs),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_realtime_query;

    #[test]
    fn restored_filters_reject_controls_spaces_in_failure_codes_and_unknown_states() {
        for (key, value) in [
            ("event_stream", "live\u{007f}stream"),
            ("event_kind", "live\tupdated"),
            ("event_kind", "\nupdated"),
            ("failure_stream", "live\u{007f}stream"),
            ("failure_consumer", "consumer\0name"),
            ("failure_consumer", "consumer\r"),
            ("failure_code", "some failure"),
            ("failure_state", "pending"),
        ] {
            let pairs = [(key.to_owned(), value.to_owned())];
            let query = parse_realtime_query(&pairs);
            assert_eq!(query.address_pairs(), pairs);
            assert_eq!(query.event_page_request().is_none(), key.starts_with("event_"));
            assert_eq!(query.failure_page_request().is_none(), key.starts_with("failure_"));
        }

        let query = parse_realtime_query(&[
            ("event_kind".to_owned(), " 状态 更新 ".to_owned()),
            ("failure_code".to_owned(), " transient_error ".to_owned()),
            ("failure_state".to_owned(), "completed".to_owned()),
        ]);
        assert_eq!(query.event_page_request().unwrap().2, "状态 更新");
        let (_, _, _, code, state) = query.failure_page_request().unwrap();
        assert_eq!(code, "transient_error");
        assert_eq!(state, "completed");
    }

    #[test]
    fn restored_duplicates_block_their_list_even_if_each_value_is_valid() {
        for (key, value) in [
            ("event_stream", "live-stream"),
            ("event_kind", "updated"),
            ("failure_stream", "live-stream"),
            ("failure_consumer", "consumer"),
            ("failure_code", "transient_error"),
            ("failure_state", "completed"),
        ] {
            let pairs = [(); 2].map(|()| (key.to_owned(), value.to_owned()));
            let query = parse_realtime_query(&pairs);
            assert_eq!(query.address_pairs(), pairs);
            assert_eq!(query.event_page_request().is_none(), key.starts_with("event_"));
            assert_eq!(query.failure_page_request().is_none(), key.starts_with("failure_"));
        }
    }
}
