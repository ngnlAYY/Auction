//! Admin realtime 运维页面状态与 replay 对话框。

use auction_web_shared::{AsyncState, AsyncStateKind};
use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;

use super::super::poll::{AutoPollControls, use_auto_poll};
use super::super::query::use_realtime_query_state;
use super::event::RealtimeEventTable;
use super::failure::{
    FailureReadRetry, FailureReads, RealtimeFailureTable, retry_failure_read, use_failure_reads,
};
use super::filter_field::fetch_filtered;
use super::filters::{RealtimeEventFilters, RealtimeFailureFilters, RealtimeFailureStateFilter};
use super::lag::RealtimeLagTable;
use super::overview::{
    activity_count_state, consumer_count_state, failure_count_state, metric_card, metric_value,
};
use crate::api::{
    RealtimeFailure, RealtimeFailurePage, fetch_realtime_event_page, fetch_realtime_event_total,
    fetch_realtime_lag_page, replay_realtime_failure,
};
use crate::surface::pagination::{AdminPager, use_admin_page_with_state, use_admin_pagination};
use crate::surface::pending::{PendingCommandPanel, set_if_alive, use_pending_commands};
use crate::surface::{AdminActionFeedback, admin_api_error_message};

#[derive(Clone, Copy, Default)]
struct ReplaySelection {
    processed: bool,
}

impl ReplaySelection {
    fn eligible(
        selected: &RealtimeFailure,
        snapshot: Option<&Result<RealtimeFailurePage, String>>,
    ) -> Option<bool> {
        let page = snapshot?.as_ref().ok()?;
        Some(page.failures.iter().any(|failure| {
            failure.id == selected.id
                && failure.event_id == selected.event_id
                && failure.stream_key == selected.stream_key
                && !failure.resolved
        }))
    }

    // 旧确认框一旦失效，后续刷新不再恢复它的提交资格。
    fn observe(self, eligible: Option<bool>) -> Self {
        Self { processed: self.processed || eligible == Some(false) }
    }

    fn available(self, eligible: impl FnOnce() -> Option<bool>) -> bool {
        !self.processed && eligible() == Some(true)
    }
}

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
    let mut selected_failure = use_signal(|| None::<RealtimeFailure>);
    let replay_refresh = use_signal(|| 0_u64);
    let pending =
        use_pending_commands(replay_refresh, |entity| entity.starts_with("realtime-failure:"));
    let mut schema_version = use_signal(|| "1".to_owned());
    let mut public_payload = use_signal(|| "{}".to_owned());
    let mut replay_invalid = use_signal(|| false);
    let mut replay_selection = use_signal(ReplaySelection::default);
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
    let FailureReads { page: failures, status: failures_status, unresolved: unresolved_failures } =
        use_failure_reads(
            failure_pagination.clone(),
            failure_query,
            failure_allowed,
            failure_refresh,
            replay_refresh,
        );
    // 只用当前查询的成功快照确认目标仍可重放，读取失败不能当作记录已处理。
    let replay_eligible = move || {
        let selection = selected_failure.read();
        let selected = selection.as_ref().filter(|_| failure_allowed())?;
        ReplaySelection::eligible(selected, failures.read().as_ref())
    };
    use_effect(move || {
        let eligible = replay_eligible();
        let current = *replay_selection.peek();
        let next = current.observe(eligible);
        if next.processed != current.processed {
            replay_selection.set(next);
        }
    });
    let replay_available = move || replay_selection().available(replay_eligible);
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
    let failure_read_error = match &*failure_state {
        Some(Err(error)) if failure_allowed() => Some(error),
        _ => None,
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
                retry_label: t!("admin-retry"),
                on_retry: move |_| retry_failure_read(failures_status.cloned(), failure_refresh),
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
                    replay_selection.set(ReplaySelection::default());
                    selected_failure.set(Some(failure));
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
            if let Some(failure) = selected_failure() {
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
                        div { role: "status",
                            if replay_selection().processed || replay_eligible() == Some(false) {
                                p { class: "admin-panel__note", {t!("realtime-replay-processed")} }
                            }
                        }
                        div { role: "alert",
                            if let Some(error) = failure_read_error {
                                p { class: "admin-panel__note action-feedback action-feedback--error", {admin_api_error_message(error)} }
                            }
                        }
                        if failure_read_error.is_some() {
                            FailureReadRetry { status: failures_status, refresh: failure_refresh }
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
                                disabled: replay_busy() || !replay_available(),
                                onclick: move |_| {
                                    if replay_busy() || !replay_available()
                                        || pending.contains(&format!("realtime-failure:{}", failure.id))
                                    {
                                        return;
                                    }
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
                                    // 已登记的命令只使用冻结目标、schema 与负载，原样重试无需读列表。
                                    let failure = failure.clone();
                                    let submitted = pending.submit(
                                        format!("realtime-failure:{}", failure.id),
                                        t!("realtime-replay-submit"),
                                        move |command_id| {
                                            let failure = failure.clone();
                                            let payload = payload.clone();
                                            async move {
                                                replay_realtime_failure(
                                                    &failure,
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
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use auction_contracts::{
        AdminRealtimeFailurePaginationResponse, AdminRealtimePaginationResponse,
    };
    use dioxus::dioxus_core::{DynamicNode, ElementId, Mutation, ScopeId, TemplateNode, VNode};
    use dioxus::prelude::{
        ReadableExt, UseResourceState, VirtualDom, dioxus_core, dioxus_elements, dioxus_signals,
        rsx, spawn, use_context_provider, use_hook, use_memo, use_signal,
    };
    use gloo_net::http::Method;
    use uuid::Uuid;

    use super::super::failure::{FailureReadRetry, use_failure_reads};
    use super::{ReplaySelection, outbox_page};
    use crate::api::{
        AdminApiFailure, AdminWriteFailure, RealtimeFailure, RealtimeFailurePage, RealtimeLagPage,
        Rejection, replay_realtime_failure,
    };
    use crate::surface::auctions::draft_page_tests::{
        button, click_button, has_live_text, mount_writer,
    };
    use crate::surface::pagination::use_admin_pagination;
    use crate::surface::pending::{PendingCommandPanel, use_pending_commands};
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

    const REPLAY_URL: &str =
        "/api/v1/admin/realtime/failures/00000000-0000-4000-8000-000000000027/replay";
    const REPLAY_CONFLICT: &str =
        r#"{"code":"conflict","message":"命令冲突","data":null,"request_id":null}"#;

    fn replay_target() -> RealtimeFailure {
        RealtimeFailure {
            id: "00000000-0000-4000-8000-000000000027".into(),
            event_id: "00000000-0000-4000-8000-000000000026".into(),
            stream_key: "auction:0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b".into(),
            consumer_key: "public-realtime".into(),
            code: "invalid_event".into(),
            safe_detail: "示例失败".into(),
            failed_at: "2025-11-15T09:46:40.110000Z".into(),
            resolved_at: None,
            resolved: false,
        }
    }

    #[test]
    fn realtime_replay_requires_a_new_dialog_after_confirmed_invalidation() {
        let target = replay_target();
        let snapshot = |failure| {
            Ok(RealtimeFailurePage {
                failures: vec![failure],
                pagination: AdminRealtimeFailurePaginationResponse {
                    page: 1,
                    page_size: 10,
                    has_more: false,
                    total: 1,
                },
            })
        };
        let valid = snapshot(target.clone());
        let failed = Err("storage_unavailable".into());
        let mut resolved = target.clone();
        resolved.resolved = true;
        resolved.resolved_at = Some("2025-11-15T10:00:00Z".into());
        let resolved = snapshot(resolved);
        let eligible = |snapshot| ReplaySelection::eligible(&target, Some(snapshot));
        let mut selection = ReplaySelection::default();

        for (result, available) in [
            (&valid, true),
            (&valid, true),
            (&failed, false),
            (&valid, true),
            (&resolved, false),
            (&valid, false),
        ] {
            let eligible = eligible(result);
            selection = selection.observe(eligible);
            assert_eq!(selection.available(|| eligible), available, "读取结果 {result:?}");
        }
        let reopened = ReplaySelection::default();
        assert!(reopened.available(|| eligible(&valid)));
    }

    #[test]
    fn realtime_replay_dialog_retry_reads_only_the_failure_list_and_count() {
        let (mut page, _) = mount_writer(|| {
            let refresh = use_signal(|| 0_u64);
            let replay_refresh = use_signal(|| 0_u64);
            let status = use_signal(|| UseResourceState::Ready);
            let pagination = use_admin_pagination(
                "realtime-failures",
                &["failure_page_size"],
                Some(200),
                1_000_000,
            );
            let query = use_memo(|| {
                Some((2, String::new(), "public-realtime".into(), String::new(), String::new()))
            });
            let allowed = use_memo(move || query.read().is_some());
            let _reads = use_failure_reads(pagination, query, allowed, refresh, replay_refresh);
            rsx! { FailureReadRetry { status, refresh } }
        });
        let expected = [
            "/api/v1/admin/realtime/failures?page=1&page_size=10&resolved=false",
            "/api/v1/admin/realtime/failures?page=2&page_size=10&consumer_key=public-realtime",
        ];
        let mut initial = page.take_gets();
        initial.sort();
        assert_eq!(initial, expected);
        click_button(&mut page, "重试");
        let mut retried = page.take_gets();
        retried.sort();
        assert_eq!(retried, expected);
    }

    #[test]
    fn realtime_replay_distinguishes_unsent_json_from_verified_conflict() {
        let outcome = Rc::new(Cell::new(None));
        let reply = outcome.clone();
        let _mounted = mount_writer(move || {
            let reply = reply.clone();
            use_hook(move || {
                AdminApiFailure::stub_write_response(
                    REPLAY_URL,
                    Method::POST,
                    409,
                    REPLAY_CONFLICT,
                );
                spawn(async move {
                    let target = replay_target();
                    let id = Uuid::parse_str("00000000-0000-4000-8000-000000000029").unwrap();
                    let unsent =
                        replay_realtime_failure(&target, id, 3, "{".into()).await.unwrap_err();
                    let conflict =
                        replay_realtime_failure(&target, id, 3, r#"{"checked":true}"#.into())
                            .await
                            .unwrap_err();
                    reply.set(Some((unsent, conflict)));
                });
            });
            VNode::empty()
        });
        let (unsent, conflict) = outcome.take().expect("两次尝试都应返回回执裁定");
        assert!(matches!(unsent.classify(), Rejection::ThisAttempt));
        assert_eq!(unsent.display_code(), "invalid_request");
        assert_eq!(unsent.observed_response_status(), None);
        assert!(matches!(conflict.classify(), Rejection::ReplayChecked));
        assert_eq!(conflict.display_code(), "conflict");
        assert_eq!(conflict.observed_response_status(), Some(409));
    }

    #[test]
    fn realtime_replay_reuses_its_command_id_until_a_verified_conflict_settles_it() {
        let attempts = Rc::new(RefCell::new(Vec::new()));
        let (mut page, _) = mount_writer({
            let attempts = attempts.clone();
            move || {
                let target = replay_target();
                let entity = format!("realtime-failure:{}", target.id);
                let attempts = attempts.clone();
                let refresh = use_signal(|| 0_u64);
                let pending =
                    use_pending_commands(refresh, |key| key.starts_with("realtime-failure:"));
                let label = "确认重放";
                rsx! {
                    PendingCommandPanel { owner: pending }
                    button {
                        disabled: pending.contains(&entity),
                        onclick: move |_| {
                            let target = target.clone();
                            let attempts = attempts.clone();
                            pending.submit(entity.clone(), label.into(),
                                move |id| {
                                    attempts.borrow_mut().push(id);
                                    let target = target.clone();
                                    async move {
                                        replay_realtime_failure(&target, id, 3, r#"{"checked":true}"#.into()).await
                                    }
                                },
                                |_| panic!("冲突流程不应交付成功回执"),
                                |_| (),
                            );
                        },
                        "{label}"
                    }
                }
            }
        });
        page.dom().in_scope(ScopeId::APP, || {
            AdminApiFailure::stub_write_response(REPLAY_URL, Method::POST, 503,
                r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#);
        });
        click_button(&mut page, "确认重放");
        assert!(button(&page, "确认重放").unwrap().1);
        let first = attempts.borrow()[0];

        page.dom().in_scope(ScopeId::APP, || {
            AdminApiFailure::stub_write_response(REPLAY_URL, Method::POST, 409, REPLAY_CONFLICT);
        });
        click_button(&mut page, "原样重试");
        assert_eq!(*attempts.borrow(), [first, first]);
        assert!(button(&page, "关闭已拒绝操作").is_some());
        assert!(!button(&page, "确认重放").unwrap().1);
    }

    #[test]
    fn realtime_replay_host_keeps_unknown_and_delivers_the_matching_result_once() {
        let attempts = Rc::new(RefCell::new(Vec::new()));
        let delivered = Rc::new(Cell::new(0));
        let (mut page, _) = mount_writer({
            let attempts = attempts.clone();
            let delivered = delivered.clone();
            move || {
                let entity = format!("realtime-failure:{}", replay_target().id);
                let refresh = use_signal(|| 0_u64);
                let pending =
                    use_pending_commands(refresh, |key| key.starts_with("realtime-failure:"));
                let attempts = attempts.clone();
                let delivered = delivered.clone();
                let label = "确认重放";
                rsx! {
                    PendingCommandPanel { owner: pending }
                    button {
                        disabled: pending.contains(&entity),
                        onclick: move |_| {
                            let attempts = attempts.clone();
                            let delivered = delivered.clone();
                            pending.submit(entity.clone(), "重放".into(), move |id| {
                                let mut attempts = attempts.borrow_mut();
                                attempts.push(id);
                                let result = match attempts.len() {
                                    1 => Err(AdminWriteFailure::outcome_unknown("invalid_response")),
                                    2 => Ok(()),
                                    _ => panic!("已了结的重放不能再次执行"),
                                };
                                std::future::ready(result)
                            }, move |()| delivered.set(delivered.get() + 1), |_| ());
                        },
                        "{label}"
                    }
                }
            }
        });
        click_button(&mut page, "确认重放");
        assert!(button(&page, "确认重放").unwrap().1);
        assert_eq!(delivered.get(), 0);
        let first = attempts.borrow()[0];

        click_button(&mut page, "原样重试");
        page.rerender();
        assert_eq!(*attempts.borrow(), [first, first]);
        assert!(button(&page, "原样重试").is_none());
        assert!(!button(&page, "确认重放").unwrap().1);
        assert_eq!(delivered.get(), 1);
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
