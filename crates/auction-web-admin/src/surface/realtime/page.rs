//! Admin realtime 运维页面状态与 replay 对话框。

use auction_web_shared::{AsyncState, AsyncStateKind};
use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;

use super::super::poll::{AutoPollControls, use_auto_poll};
use super::super::query::use_realtime_query_state;
use super::event::RealtimeEventTable;
use super::failure::RealtimeFailureTable;
use super::filter_field::fetch_filtered;
use super::filters::{RealtimeEventFilters, RealtimeFailureFilters, RealtimeFailureStateFilter};
use super::lag::RealtimeLagTable;
use super::overview::{
    activity_count_state, consumer_count_state, failure_count_state, metric_card, metric_value,
};
use crate::api::{
    RealtimeFailure, fetch_realtime_event_page, fetch_realtime_event_total,
    fetch_realtime_failure_page, fetch_realtime_lag_page, replay_realtime_failure,
};
use crate::surface::pagination::{AdminPager, use_admin_page_with_state, use_admin_pagination};
use crate::surface::pending::{PendingCommandPanel, set_if_alive, use_pending_commands};
use crate::surface::{AdminActionFeedback, admin_api_error_message};

/// Outbox 路由展示已持久化事件元数据与 realtime failure/lag/replay 安全投影；未冻结的外部
/// 广播 sink 不在页面中虚构状态或成功结果。
pub(crate) fn outbox_page() -> Element {
    page_with_copy(t!("page-outbox-title"), t!("page-outbox-note"))
}

fn page_with_copy(page_title: String, page_note: String) -> Element {
    let mut event_refresh = use_signal(|| 0_u64);
    let mut failure_refresh = use_signal(|| 0_u64);
    let mut lag_refresh = use_signal(|| 0_u64);
    let mut notice = use_signal(|| None::<AdminActionFeedback>);
    let mut selected_failure = use_signal(|| None::<String>);
    let replay_refresh = use_signal(|| 0_u64);
    let pending =
        use_pending_commands(replay_refresh, |entity| entity.starts_with("realtime-failure:"));
    let mut schema_version = use_signal(|| "1".to_owned());
    let mut public_payload = use_signal(|| "{}".to_owned());
    let mut replay_invalid = use_signal(|| false);
    let replay_busy = move || pending.any_busy();
    // 已应用的事件/failure 筛选与三个分区的分页跟地址栏双向同步；筛选输入框草稿（留在
    // 筛选组件本地）、replay 对话框与负载输入是瞬态，不入 URL。
    let realtime_query = use_realtime_query_state();
    let query = realtime_query.location;
    let query_state = query.state;
    let event_pagination =
        use_admin_pagination("realtime-events", &["event_page_size"], Some(200), 1_000_000);
    let failure_pagination =
        use_admin_pagination("realtime-failures", &["failure_page_size"], Some(200), 1_000_000);
    let lag_pagination =
        use_admin_pagination("realtime-lag", &["lag_page_size"], Some(200), 1_000_000);
    // 三个分区只订阅自己的请求身份；另一区翻页或切换轮询间隔不重发无关 GET。
    let event_query = use_memo(move || query_state.read().event_page_request());
    let lag_query = use_memo(move || query_state.read().lag_page);
    let failure_query = use_memo(move || query_state.read().failure_page_request());
    let event_allowed = use_memo(move || event_query.read().is_some());
    let failure_allowed = use_memo(move || failure_query.read().is_some());
    let (events, events_status) = use_admin_page_with_state(
        event_pagination.clone(),
        move || {
            let _ = replay_refresh();
            let _ = event_refresh();
            event_query()
        },
        |request, size| {
            fetch_filtered(request, move |(page, stream, kind)| {
                fetch_realtime_event_page(page, stream, kind, size)
            })
        },
    );
    let event_overview = use_resource(move || {
        let _ = replay_refresh();
        let _ = event_refresh();
        fetch_filtered(event_allowed().then_some(()), |()| fetch_realtime_event_total())
    });
    let (lag, lag_status) = use_admin_page_with_state(
        lag_pagination.clone(),
        move || {
            let _ = replay_refresh();
            let _ = lag_refresh();
            lag_query()
        },
        |page, size| async move {
            #[cfg(test)]
            if let Some(reads) = try_consume_context::<std::rc::Rc<tests::LagReads>>() {
                return reads.fetch(page, size);
            }
            fetch_realtime_lag_page(page, String::new(), String::new(), size).await
        },
    );
    let (failures, failures_status) = use_admin_page_with_state(
        failure_pagination.clone(),
        move || {
            let _ = replay_refresh();
            let _ = failure_refresh();
            failure_query()
        },
        |request, size| {
            fetch_filtered(request, move |(page, stream, consumer, code, state)| {
                fetch_realtime_failure_page(page, stream, consumer, code, state, size)
            })
        },
    );
    let unresolved_failures = use_resource(move || {
        let _ = replay_refresh();
        let _ = failure_refresh();
        fetch_filtered(failure_allowed().then_some(()), |()| {
            fetch_realtime_failure_page(
                1,
                String::new(),
                String::new(),
                String::new(),
                "dead_letter".to_owned(),
                10,
            )
        })
    });
    // 各分区只在自己的读取（含配套计数）完成后轮询；慢请求不能被下一节拍取消。
    let poll_seconds = use_memo(move || query_state.read().poll_seconds);
    use_auto_poll(poll_seconds, move || {
        if event_allowed()
            && events_status.cloned() == UseResourceState::Ready
            && event_overview.state().cloned() == UseResourceState::Ready
        {
            event_refresh.set(event_refresh().saturating_add(1));
        }
        if lag_status.cloned() == UseResourceState::Ready {
            lag_refresh.set(lag_refresh().saturating_add(1));
        }
        if failure_allowed()
            && failures_status.cloned() == UseResourceState::Ready
            && unresolved_failures.state().cloned() == UseResourceState::Ready
        {
            failure_refresh.set(failure_refresh().saturating_add(1));
        }
    });

    let mut load_events_query = query.clone();
    let load_events = move |filters| {
        // 应用草稿筛选并回到第一页；事件分区资源响应查询状态变化自动重新加载。
        load_events_query.push(query_state().with_event_filters(filters));
    };
    let mut load_failures_query = query.clone();
    let load_failures = move |filters| {
        load_failures_query.push(query_state().with_failure_filters(filters));
        selected_failure.set(None);
    };
    // 顶部手动刷新、轮询开关与 failure 状态选择各持一份 hook 克隆。
    let mut refresh_query = query.clone();
    let mut poll_query = query.clone();
    let mut state_query = query.clone();
    // 直接借用已完成快照；表格取得所需投影，统计与分页不保留额外整页副本。
    let event_state = events.read_unchecked();
    let event_overview_state = event_overview.read_unchecked();
    let lag_state = lag.read_unchecked();
    let failure_state = failures.read_unchecked();
    let unresolved_failure_state = unresolved_failures.read_unchecked();
    let lag_page = (*lag_state).as_ref().and_then(|result| result.as_ref().ok());
    let unresolved_failure_page = (*unresolved_failure_state)
        .as_ref()
        .filter(|_| failure_allowed())
        .and_then(|result| result.as_ref().ok());
    let event_total = (*event_overview_state)
        .as_ref()
        .filter(|_| event_allowed())
        .and_then(|result| result.as_ref().ok())
        .copied();
    let consumer_total = lag_page.map(|page| page.pagination.total);
    let failure_total = unresolved_failure_page.map(|page| page.pagination.total);
    let selected_id = selected_failure();
    let selected = (*failure_state)
        .as_ref()
        .filter(|_| failure_allowed())
        .and_then(|result| result.as_ref().ok())
        .and_then(|page| {
            selected_id
                .as_ref()
                .and_then(|id| page.failures.iter().find(|failure| &failure.id == id).cloned())
        });
    let event_content = match &*event_state {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("realtime-loading"),
                detail: "",
            }
        },
        Some(Err(error)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: admin_api_error_message(error),
                detail: "",
                // 错误态提供页内重试入口：仅重载事件分区（含总览计数）。
                retry_label: t!("admin-retry"),
                on_retry: move |_| event_refresh.set(event_refresh().saturating_add(1)),
            }
        },
        Some(Ok(event_page)) => rsx! {
            RealtimeEventTable { page: event_page.clone() }
        },
    };
    let event_pager = match &*event_state {
        Some(Ok(event_page)) => {
            let mut on_page_change = query.clone();
            rsx! {
                AdminPager {
                    pagination: event_pagination,
                    page: event_page.pagination.page,
                    page_size: event_page.pagination.page_size,
                    total: Some(event_page.pagination.total),
                    label: t!("realtime-event-title"),
                    on_page_change: move |next_page| {
                        let mut next = query_state();
                        next.event_page = next_page;
                        on_page_change.push(next);
                    },
                }
            }
        }
        _ => rsx! {},
    };
    let failure_content = match &*failure_state {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("realtime-loading"),
                detail: "",
            }
        },
        Some(Err(error)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: admin_api_error_message(error),
                detail: "",
                // 错误态提供页内重试入口：仅重载 failure 分区（含未解决计数）。
                retry_label: t!("admin-retry"),
                on_retry: move |_| failure_refresh.set(failure_refresh().saturating_add(1)),
            }
        },
        Some(Ok(failure_page)) => rsx! {
            RealtimeFailureTable {
                page: failure_page.clone(),
                on_select: move |failure: RealtimeFailure| {
                    if replay_busy() || pending.contains(&format!("realtime-failure:{}", failure.id)) { return; }
                    schema_version.set("1".to_owned());
                    public_payload.set("{}".to_owned());
                    replay_invalid.set(false);
                    selected_failure.set(Some(failure.id));
                    notice.set(None);
                },
            }
        },
    };
    let failure_pager = match &*failure_state {
        Some(Ok(failure_page)) => {
            let mut on_page_change = query.clone();
            rsx! {
                AdminPager {
                    pagination: failure_pagination,
                    page: failure_page.pagination.page,
                    page_size: failure_page.pagination.page_size,
                    total: Some(failure_page.pagination.total),
                    label: t!("realtime-failure-title"),
                    on_page_change: move |next_page| {
                        let mut next = query_state();
                        next.failure_page = next_page;
                        on_page_change.push(next);
                    },
                }
            }
        }
        _ => rsx! {},
    };
    let lag_content = match &*lag_state {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("realtime-loading"),
                detail: "",
            }
        },
        Some(Err(error)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: admin_api_error_message(error),
                detail: "",
                // 错误态提供页内重试入口：仅重载 lag 分区。
                retry_label: t!("admin-retry"),
                on_retry: move |_| lag_refresh.set(lag_refresh().saturating_add(1)),
            }
        },
        Some(Ok(lag_page)) => {
            let mut on_page_change = query;
            rsx! {
                RealtimeLagTable { page: lag_page.clone() }
                AdminPager {
                    pagination: lag_pagination,
                    page: lag_page.pagination.page,
                    page_size: lag_page.pagination.page_size,
                    total: Some(lag_page.pagination.total),
                    label: t!("realtime-lag-title"),
                    on_page_change: move |next_page| {
                        let mut next = query_state();
                        next.lag_page = next_page;
                        on_page_change.push(next);
                    },
                }
            }
        }
    };

    rsx! {
        section { class: "admin-realtime-page", aria_labelledby: "outbox-title",
            PendingCommandPanel { owner: pending }
            div { class: "admin-realtime-heading",
                h1 { id: "outbox-title", {page_title} }
                div { class: "admin-realtime-heading__actions",
                    AutoPollControls {
                        poll_seconds: poll_seconds(),
                        on_change: move |seconds| {
                            // 开关与间隔变更立即生效并写回地址栏。
                            let mut next = query_state();
                            next.poll_seconds = seconds;
                            poll_query.push(next);
                        },
                    }
                    button {
                    r#type: "button",
                    class: "admin-btn-secondary",
                    onclick: move |_| {
                        selected_failure.set(None);
                        notice.set(None);
                        // 手动刷新把三个分区都带回第一页并写回地址栏。
                        let mut next = query_state();
                        next.event_page = 1;
                        next.lag_page = 1;
                        next.failure_page = 1;
                        refresh_query.push(next);
                        event_refresh.set(event_refresh().saturating_add(1));
                        lag_refresh.set(lag_refresh().saturating_add(1));
                        failure_refresh.set(failure_refresh().saturating_add(1));
                    },
                    {t!("realtime-refresh")}
                    }
                }
            }
            p { class: "admin-realtime-intro", {page_note} }
            div {
                class: "admin-realtime-overview",
                role: "region",
                aria_label: t!("realtime-overview-title"),
                {metric_card(
                    t!("realtime-overview-events"),
                    metric_value(event_total),
                    activity_count_state(event_total),
                )}
                {metric_card(
                    t!("realtime-overview-consumers"),
                    metric_value(consumer_total),
                    consumer_count_state(consumer_total),
                )}
                {metric_card(
                    t!("realtime-overview-failures"),
                    metric_value(failure_total),
                    failure_count_state(failure_total),
                )}
            }

            section { class: "admin-realtime-section", aria_labelledby: "realtime-events-title",
                div { class: "admin-realtime-section__header",
                    div {
                        h2 { id: "realtime-events-title", {t!("realtime-event-title")} }
                        p { {t!("realtime-event-note")} }
                    }
                }
                RealtimeEventFilters {
                    applied: realtime_query.event_filters,
                    invalid_applied: !event_allowed(),
                    on_load: load_events,
                }
                if event_allowed() {
                    {event_content}
                    {event_pager}
                }
            }

            section { class: "admin-realtime-section", aria_labelledby: "realtime-lag-title",
                div { class: "admin-realtime-section__header",
                    div {
                        h2 { id: "realtime-lag-title", {t!("realtime-lag-title")} }
                        p { {t!("realtime-lag-note")} }
                    }
                }
                {lag_content}
            }

            section { class: "admin-realtime-section", aria_labelledby: "realtime-failures-title",
                div { class: "admin-realtime-section__header",
                    div {
                        h2 { id: "realtime-failures-title", {t!("realtime-failure-title")} }
                        p { {t!("realtime-failure-note")} }
                    }
                    RealtimeFailureStateFilter {
                        value: query_state().failure_state,
                        on_change: move |value: String| {
                            let mut next = query_state();
                            next.failure_state = value.into();
                            next.failure_page = 1;
                            state_query.push(next);
                            selected_failure.set(None);
                        },
                    }
                }
                if failure_allowed() {
                    {failure_content}
                }
                if let Some(feedback) = notice() {
                    p {
                        class: if feedback.is_success() {
                            "admin-panel__note action-feedback action-feedback--success"
                        } else {
                            "admin-panel__note action-feedback action-feedback--error"
                        },
                        role: if feedback.is_success() { "status" } else { "alert" },
                        {feedback.message(t!("realtime-replay-success"))}
                    }
                }
                if failure_allowed() {
                    {failure_pager}
                }
                RealtimeFailureFilters {
                    applied: realtime_query.failure_filters,
                    invalid_applied: !failure_allowed(),
                    on_load: load_failures,
                }
            }
            if let Some(failure) = selected {
                // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管；.admin-realtime-dialog
                // 内容样式挂在面板内的包装 div；共享组件暂不支持 aria_describedby。
                Dialog {
                    title: t!("realtime-replay-title", id: failure.id.clone()),
                    close_label: t!("realtime-replay-cancel"),
                    busy: replay_busy(),
                    on_close: move |_| {
                        selected_failure.set(None);
                    },
                    div { class: "admin-realtime-dialog",
                        p {
                            class: "admin-panel__note",
                            {t!("realtime-replay-warning", event_id: failure.event_id.clone())}
                        }
                        label {
                            {t!("realtime-replay-schema")}
                            input {
                                r#type: "number",
                                min: "1",
                                step: "1",
                                autofocus: true,
                                disabled: replay_busy(),
                                value: schema_version(),
                                oninput: move |event| {
                                    if replay_busy() { return; }
                                    schema_version.set(event.value());
                                    replay_invalid.set(false);
                                },
                            }
                        }
                        label {
                            {t!("realtime-replay-payload")}
                            textarea {
                                spellcheck: "false",
                                disabled: replay_busy(),
                                value: public_payload(),
                                oninput: move |event| {
                                    if replay_busy() { return; }
                                    public_payload.set(event.value());
                                    replay_invalid.set(false);
                                },
                            }
                        }
                        if replay_invalid() {
                            p { role: "alert", {t!("realtime-replay-invalid")} }
                        }
                        div { class: "admin-shell__dialog-actions",
                            button {
                                r#type: "button",
                                disabled: replay_busy(),
                                onclick: move |_| {
                                    if replay_busy() || pending.contains(&format!("realtime-failure:{}", failure.id)) {
                                        return;
                                    }
                                    let failure_id = failure.id.clone();
                                    let schema_input = schema_version();
                                    let payload = public_payload();
                                    let schema = schema_input.parse::<i32>();
                                    let valid_payload = serde_json::from_str::<serde_json::Value>(&payload)
                                        .is_ok_and(|value| value.is_object());
                                    let Ok(schema) = schema else {
                                        replay_invalid.set(true);
                                        return;
                                    };
                                    if schema <= 0 || !valid_payload {
                                        replay_invalid.set(true);
                                        return;
                                    }
                                    replay_invalid.set(false);
                                    notice.set(None);
                                    let submitted = pending.submit(
                                        format!("realtime-failure:{failure_id}"),
                                        t!("realtime-replay-submit"),
                                        move |command_id| {
                                            let failure_id = failure_id.clone();
                                            let payload = payload.clone();
                                            async move {
                                                replay_realtime_failure(
                                                    failure_id,
                                                    command_id,
                                                    schema,
                                                    payload,
                                                )
                                                .await
                                            }
                                        },
                                        move |()| {
                                            set_if_alive(notice, Some(AdminActionFeedback::Success));
                                        },
                                        move |code| {
                                            set_if_alive(notice, Some(AdminActionFeedback::Error(code)));
                                        },
                                    );
                                    if submitted {
                                        selected_failure.set(None);
                                    }
                                },
                                {t!("realtime-replay-submit")}
                            }
                            button {
                                r#type: "button",
                                class: "admin-btn-secondary",
                                disabled: replay_busy(),
                                onclick: move |_| {
                                    selected_failure.set(None);
                                },
                                {t!("realtime-replay-cancel")}
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use auction_contracts::AdminRealtimePaginationResponse;
    use dioxus::dioxus_core::{DynamicNode, ElementId, Mutation, TemplateNode, VNode};
    use dioxus::prelude::{VirtualDom, use_context_provider};

    use super::outbox_page;
    use crate::api::{AdminApiFailure, RealtimeLagPage};
    use crate::surface::auctions::draft_page_tests::{click_button, has_live_text, mount_writer};
    use crate::surface::realtime::filters::{EVENT_FILTER_FORM_ID, FAILURE_FILTER_FORM_ID};
    use crate::surface::realtime::{
        TestPage, get_parameter, selected_options, submit_forms, text_attributes,
    };

    const EVENTS: &str = "/api/v1/admin/realtime/events?";
    const FAILURES: &str = "/api/v1/admin/realtime/failures?";

    #[derive(Default)]
    pub(super) struct LagReads(RefCell<Vec<(i64, i64)>>);

    impl LagReads {
        pub(super) fn fetch(&self, page: i64, page_size: i64) -> Result<RealtimeLagPage, String> {
            let mut reads = self.0.borrow_mut();
            reads.push((page, page_size));
            match reads.len() {
                1 => Err(AdminApiFailure::from_http_body(
                    503,
                    r#"{"code":"storage_unavailable","message":"存储暂不可用","data":null,"request_id":null}"#,
                ).code),
                2 => Ok(RealtimeLagPage {
                    lags: Vec::new(),
                    pagination: AdminRealtimePaginationResponse { page, page_size, total: 0 },
                }),
                _ => panic!("发生了额外的消费延迟读取"),
            }
        }
    }

    #[test]
    fn realtime_retries_failed_read_after_two_page_renders() {
        let reads = Rc::new(LagReads::default());
        let (mut page, _) = mount_writer({
            let reads = reads.clone();
            move || {
                use_context_provider(|| reads.clone());
                outbox_page()
            }
        });
        let failed = "所需服务暂时不可用。请稍后再试。";
        let empty = "当前筛选没有匹配的消费延迟。";
        assert!(has_live_text(&page, failed));
        assert!(!has_live_text(&page, empty));
        page.rerender();
        page.rerender();
        let first = reads.0.borrow()[0];
        assert_eq!(*reads.0.borrow(), [first]);
        click_button(&mut page, "重试");
        assert_eq!(*reads.0.borrow(), [first; 2]);
        assert!(has_live_text(&page, empty));
        assert!(!has_live_text(&page, failed));
    }

    #[derive(Clone, Copy)]
    struct TextFilter {
        address_key: &'static str,
        input_id: &'static str,
        form_id: &'static str,
        endpoint: &'static str,
        parameter: &'static str,
        overview_size: &'static str,
    }

    const TEXT_FILTERS: [TextFilter; 2] = [
        TextFilter {
            address_key: "event_kind",
            input_id: "realtime-event-kind",
            form_id: EVENT_FILTER_FORM_ID,
            endpoint: EVENTS,
            parameter: "kind",
            overview_size: "1",
        },
        TextFilter {
            address_key: "failure_consumer",
            input_id: "realtime-failure-consumer",
            form_id: FAILURE_FILTER_FORM_ID,
            endpoint: FAILURES,
            parameter: "consumer_key",
            overview_size: "10",
        },
    ];

    const FILTER_KEYS: [&str; 5] =
        ["event_stream", "event_kind", "failure_stream", "failure_consumer", "failure_code"];

    fn location(label: &str, event_page: i64, failure_page: i64) -> Vec<(String, String)> {
        let mut pairs =
            FILTER_KEYS.map(|key| (key.to_owned(), format!(" {label}-{key} "))).to_vec();
        pairs.push(("event_page".to_owned(), event_page.to_string()));
        pairs.push(("failure_page".to_owned(), failure_page.to_string()));
        pairs
    }

    fn mount_form(field: TextFilter) -> (TestPage, ElementId, ElementId) {
        let (page, edits) =
            TestPage::mount(outbox_page, "event_kind=initial&failure_consumer=initial");
        let by_id = |name| {
            text_attributes(&edits, "id")
                .find_map(|(id, actual)| (actual == name).then_some(id))
                .unwrap()
        };
        let form = by_id(field.form_id);
        assert!(submit_forms(&edits).any(|target| target == form));
        let input = by_id(field.input_id);
        let initial = page.take_gets();
        assert!(
            initial.iter().any(|url| {
                url.starts_with(field.endpoint)
                    && get_parameter(url, field.parameter).as_deref() == Some("initial")
            }),
            "{initial:?}"
        );
        (page, form, input)
    }

    fn assert_list_get(gets: &[String], field: TextFilter, value: &str) {
        let lists: Vec<_> = gets
            .iter()
            .filter(|url| {
                url.starts_with(field.endpoint) && get_parameter(url, field.parameter).is_some()
            })
            .collect();
        assert_eq!(lists.len(), 1, "{gets:?}");
        assert_eq!(get_parameter(lists[0], field.parameter).as_deref(), Some(value), "{gets:?}");
    }

    fn assert_error(edits: &[Mutation], input: ElementId) {
        assert!(text_attributes(edits, "aria-invalid").any(|entry| entry == (input, "true")));
        assert!(
            text_attributes(edits, "aria-describedby")
                .any(|(id, description)| id == input && description.ends_with("-error"))
        );
        assert!(edits.iter().any(|edit| matches!(edit,
            Mutation::CreateTextNode { value, .. } if value == "请求参数无效。请检查输入内容后重试。"
        )));
    }

    #[test]
    fn failure_status_bookmarks_select_the_option_used_by_the_first_list_get() {
        for (state, resolved) in [("dead_letter", "false"), ("completed", "true")] {
            // 消费者筛选用于区分列表与同一端点的全局失败概览。
            let query = format!("failure_state={state}&failure_consumer=consumer");
            let (page, edits) = TestPage::mount(outbox_page, &query);
            let gets = page.take_gets();
            assert_eq!(gets.iter().filter(|url| url.starts_with(FAILURES)).count(), 2, "{gets:?}");
            let list = gets
                .iter()
                .find(|url| {
                    url.starts_with(FAILURES)
                        && get_parameter(url, "consumer_key").as_deref() == Some("consumer")
                })
                .unwrap();
            assert_eq!(get_parameter(list, "resolved").as_deref(), Some(resolved));
            assert_eq!(selected_options(&edits), [state]);
        }
    }

    #[test]
    fn invalid_bookmarks_skip_both_lists_and_their_overviews_on_mount() {
        let (page, _) =
            TestPage::mount(outbox_page, "event_kind=a%0Ab&failure_consumer=&failure_consumer=x");
        let gets = page.take_gets();
        assert_eq!(gets.len(), 1, "只应读取无关的 lag 分区：{gets:?}");
        assert!(gets[0].starts_with("/api/v1/admin/realtime/lag?"), "{gets:?}");
    }

    #[test]
    fn realtime_forms_restore_history_and_preserve_drafts_on_pagination() {
        let (mut page, setup) = TestPage::mount(outbox_page, "");
        let forms: Vec<_> = submit_forms(&setup).collect();
        assert_eq!(forms.len(), 2);
        let initial = page.restore(location("first", 1, 1));
        let inputs = FILTER_KEYS.map(|key| {
            let expected = format!("first-{key}");
            text_attributes(&initial, "value")
                .find_map(|(input, value)| (value == expected).then_some(input))
                .unwrap_or_else(|| panic!("缺少 {expected} 对应的筛选控件更新"))
        });
        let [event_stream, _, _, failure_consumer, _] = inputs;
        for (input, value) in
            [(event_stream, " event draft "), (failure_consumer, " consumer draft ")]
        {
            let typed = page.dispatch("input", input, value);
            assert!(text_attributes(&typed, "value").any(|entry| entry == (input, value)));
        }
        for (event_page, failure_page) in [(2, 1), (2, 3), (2, 3)] {
            let paged = page.restore(location("first", event_page, failure_page));
            assert!(!text_attributes(&paged, "value").any(|(input, _)| inputs.contains(&input)));
        }
        page.take_gets();
        for form in forms {
            page.dispatch("submit", form, "");
        }
        let gets = page.take_gets();
        let event = gets.iter().find(|url| url.starts_with(EVENTS)).unwrap();
        assert_eq!(get_parameter(event, "stream_key").as_deref(), Some("event draft"));
        assert_list_get(&gets, TEXT_FILTERS[0], "first-event_kind");
        assert_list_get(&gets, TEXT_FILTERS[1], "consumer draft");

        for label in ["second", "first", "second"] {
            let restored = page.restore(location(label, 1, 1));
            for (input, key) in inputs.into_iter().zip(FILTER_KEYS) {
                let expected = format!("{label}-{key}");
                assert!(
                    text_attributes(&restored, "value")
                        .any(|entry| entry == (input, expected.as_str()))
                );
            }
        }
    }

    #[test]
    fn realtime_forms_round_trip_128_bytes_and_reject_129_on_submit() {
        let valid = format!("{}ab", "汉".repeat(42));
        let too_long = "汉".repeat(43);
        assert_eq!((valid.len(), too_long.len()), (128, 129));
        for field in TEXT_FILTERS {
            let (mut page, form, input) = mount_form(field);
            let initial = page.address();
            page.dispatch("input", input, &format!(" {valid} "));
            page.dispatch("submit", form, "");
            let applied = page.address();
            assert!(applied.contains(&(field.address_key.to_owned(), valid.clone())));
            assert_list_get(&page.take_gets(), field, &valid);
            page.restore(initial);
            page.take_gets();
            let restored = page.restore(applied.clone());
            assert!(
                text_attributes(&restored, "value").any(|entry| entry == (input, valid.as_str()))
            );
            assert_list_get(&page.take_gets(), field, &valid);

            page.dispatch("input", input, &too_long);
            let rejected = page.dispatch("submit", form, "");
            assert_eq!(page.address(), applied);
            assert_error(&rejected, input);
            page.assert_no_gets();
        }
    }

    fn assert_invalid_restore(field: TextFilter, originals: &[&str]) {
        let (mut page, form, input) = mount_form(field);
        let mut address = page.address();
        address.retain(|(key, _)| key != field.address_key);
        address.extend(
            originals.iter().map(|value| (field.address_key.to_owned(), (*value).to_owned())),
        );
        let restored = page.restore(address);
        page.assert_no_gets();
        assert_error(&restored, input);
        for original in originals {
            let escaped = format!("{original:?}");
            let count = restored
                .iter()
                .filter(|edit| {
                    matches!(edit,
                        Mutation::CreateTextNode { value, .. } if *value == escaped
                    )
                })
                .count();
            assert_eq!(
                count,
                originals.iter().filter(|value| value == &original).count(),
                "{escaped}"
            );
        }
        if originals.iter().any(|value| value.contains('\n')) {
            assert!(restored.iter().any(|edit| matches!(edit,
                Mutation::CreateTextNode { value, .. } if value.contains("U+000A")
            )));
        }

        let invalid_address = page.address();
        page.dispatch("submit", form, "");
        assert_eq!(page.address(), invalid_address);
        page.assert_no_gets();
        page.dispatch("input", input, "fixed");
        assert_eq!(page.address(), invalid_address);
        page.assert_no_gets();
        page.dispatch("submit", form, "");
        assert_eq!(
            page.address().iter().filter(|(key, _)| key == field.address_key).collect::<Vec<_>>(),
            [&(field.address_key.to_owned(), "fixed".to_owned())]
        );
        let gets = page.take_gets();
        assert_eq!(gets.len(), 2, "{gets:?}");
        assert_list_get(&gets, field, "fixed");
        assert!(
            gets.iter().any(|url| {
                url.starts_with(field.endpoint)
                    && get_parameter(url, field.parameter).is_none()
                    && get_parameter(url, "page_size").as_deref() == Some(field.overview_size)
            }),
            "{gets:?}"
        );
    }

    #[test]
    fn repeated_filters_preserve_empty_and_identical_values_until_corrected() {
        let too_long = "汉".repeat(43);
        for field in TEXT_FILTERS {
            assert_invalid_restore(field, &["", &too_long]);
            assert_invalid_restore(field, &["same", "same"]);
        }
    }

    #[test]
    fn restored_control_characters_are_escaped_and_wait_for_a_correction() {
        for field in TEXT_FILTERS {
            assert_invalid_restore(field, &["a\nb"]);
        }
    }

    #[test]
    fn restored_overlong_values_use_the_same_error_and_wait_for_a_correction() {
        let too_long = format!(" {} ", "汉".repeat(43));
        for field in TEXT_FILTERS {
            assert_invalid_restore(field, &[&too_long]);
        }
    }

    #[test]
    fn overlong_ascii_value_renders_in_parts_without_losing_text() {
        let raw = "X".repeat(129);
        let (page, edits) = TestPage::mount(outbox_page, &format!("event_kind={raw}"));
        let parts = edits
            .iter()
            .filter_map(|edit| match edit {
                Mutation::CreateTextNode { value, .. }
                    if value.contains('X')
                        && value.chars().all(|character| matches!(character, 'X' | '"')) =>
                {
                    Some(value.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(parts.len() > 1);
        assert_eq!(parts.concat(), format!("{raw:?}"));
        let dom = page.dom();
        assert_eq!(word_breaks(dom, dom.base_scope().root_node()), parts.len() - 1);
        assert!(page.take_gets().iter().all(|url| !url.starts_with(EVENTS)));
    }

    fn word_breaks(dom: &VirtualDom, node: &VNode) -> usize {
        fn in_template(node: &TemplateNode) -> usize {
            match node {
                TemplateNode::Element { tag: "wbr", .. } => 1,
                TemplateNode::Element { children, .. } => children.iter().map(in_template).sum(),
                _ => 0,
            }
        }
        let static_count: usize = node.template.roots.iter().map(in_template).sum();
        let dynamic_count: usize = node
            .dynamic_nodes
            .iter()
            .enumerate()
            .map(|(index, child)| match child {
                DynamicNode::Component(component) => component
                    .mounted_scope(index, node, dom)
                    .and_then(|scope| scope.try_root_node())
                    .map_or(0, |root| word_breaks(dom, root)),
                DynamicNode::Fragment(nodes) => {
                    nodes.iter().map(|node| word_breaks(dom, node)).sum()
                }
                _ => 0,
            })
            .sum();
        static_count + dynamic_count
    }

    #[test]
    fn a_single_valid_filter_restores_and_queries_without_an_edit() {
        for field in TEXT_FILTERS {
            let (mut page, _, input) = mount_form(field);
            let mut address = page.address();
            address.iter_mut().find(|(key, _)| key == field.address_key).unwrap().1 =
                " valid ".to_owned();
            let restored = page.restore(address);
            assert!(text_attributes(&restored, "value").any(|entry| entry == (input, "valid")));
            assert_list_get(&page.take_gets(), field, "valid");
        }
    }
}
