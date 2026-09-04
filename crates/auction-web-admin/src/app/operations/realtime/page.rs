//! Admin realtime 运维页面状态与 replay 对话框。

use auction_web_shared::{Dialog, legacy_t as t};
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::{
    RealtimeFailure, fetch_realtime_event_page, fetch_realtime_event_total,
    fetch_realtime_failure_page, fetch_realtime_lag_page, replay_realtime_failure,
};

use super::event::{RealtimeEventPager, RealtimeEventTable};
use super::failure::{RealtimeFailurePager, RealtimeFailureTable};
use super::filters::{RealtimeEventFilters, RealtimeFailureFilters};
use super::lag::{RealtimeLagPager, RealtimeLagTable};
use crate::app::surface::{AdminActionFeedback, admin_api_error_message};

use auction_web_shared::{AsyncState, AsyncStateKind};

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
    let mut replay_command_id = use_signal(|| None::<(String, Uuid)>);
    let mut schema_version = use_signal(|| "1".to_owned());
    let mut public_payload = use_signal(|| "{}".to_owned());
    let mut replay_invalid = use_signal(|| false);
    let mut replay_busy = use_signal(|| false);
    let mut lag_page_number = use_signal(|| 1_i64);
    let mut event_page_number = use_signal(|| 1_i64);
    let mut failure_page_number = use_signal(|| 1_i64);
    let event_stream_filter = use_signal(String::new);
    let event_kind_filter = use_signal(String::new);
    let mut active_event_stream_filter = use_signal(String::new);
    let mut active_event_kind_filter = use_signal(String::new);
    let failure_stream_filter = use_signal(String::new);
    let failure_consumer_filter = use_signal(String::new);
    let failure_code_filter = use_signal(String::new);
    let mut failure_resolved_filter = use_signal(String::new);
    let mut active_failure_stream_filter = use_signal(String::new);
    let mut active_failure_consumer_filter = use_signal(String::new);
    let mut active_failure_code_filter = use_signal(String::new);
    let mut active_failure_state_filter = use_signal(String::new);
    let events = use_resource(move || {
        let _ = event_refresh();
        fetch_realtime_event_page(
            event_page_number(),
            active_event_stream_filter(),
            active_event_kind_filter(),
        )
    });
    let event_overview = use_resource(move || {
        let _ = event_refresh();
        fetch_realtime_event_total()
    });
    let lag = use_resource(move || {
        let _ = lag_refresh();
        fetch_realtime_lag_page(lag_page_number(), String::new(), String::new())
    });
    let failures = use_resource(move || {
        let _ = failure_refresh();
        fetch_realtime_failure_page(
            failure_page_number(),
            active_failure_stream_filter(),
            active_failure_consumer_filter(),
            active_failure_code_filter(),
            active_failure_state_filter(),
        )
    });
    let unresolved_failures = use_resource(move || {
        let _ = failure_refresh();
        fetch_realtime_failure_page(
            1,
            String::new(),
            String::new(),
            String::new(),
            "dead_letter".to_owned(),
        )
    });

    let load_events = move |_| {
        active_event_stream_filter.set(event_stream_filter());
        active_event_kind_filter.set(event_kind_filter());
        event_page_number.set(1);
        event_refresh.set(event_refresh().saturating_add(1));
    };
    let load_failures = move |_| {
        active_failure_stream_filter.set(failure_stream_filter());
        active_failure_consumer_filter.set(failure_consumer_filter());
        active_failure_code_filter.set(failure_code_filter());
        active_failure_state_filter.set(failure_resolved_filter());
        failure_page_number.set(1);
        selected_failure.set(None);
        replay_command_id.set(None);
        failure_refresh.set(failure_refresh().saturating_add(1));
    };
    let event_state = events.read_unchecked().clone();
    let event_overview_state = event_overview.read_unchecked().clone();
    let lag_state = lag.read_unchecked().clone();
    let failure_state = failures.read_unchecked().clone();
    let unresolved_failure_state = unresolved_failures.read_unchecked().clone();
    let lag_page = lag_state.as_ref().and_then(|result| result.as_ref().ok());
    let unresolved_failure_page =
        unresolved_failure_state.as_ref().and_then(|result| result.as_ref().ok());
    let event_total =
        event_overview_state.as_ref().and_then(|result| result.as_ref().ok()).copied();
    let consumer_total = lag_page.map(|page| page.pagination.total);
    let failure_total = unresolved_failure_page.map(|page| page.pagination.total);
    let selected_id = selected_failure();
    let selected =
        failure_state.as_ref().and_then(|result| result.as_ref().ok()).and_then(|page| {
            selected_id
                .as_ref()
                .and_then(|id| page.failures.iter().find(|failure| &failure.id == id).cloned())
        });
    let event_content = match &event_state {
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
            }
        },
        Some(Ok(event_page)) => rsx! {
            RealtimeEventTable { page: event_page.clone() }
        },
    };
    let event_pager = match &event_state {
        Some(Ok(event_page)) => rsx! {
            RealtimeEventPager { page: event_page.clone(), page_number: event_page_number }
        },
        _ => rsx! {},
    };
    let failure_content = match &failure_state {
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
            }
        },
        Some(Ok(failure_page)) => rsx! {
            RealtimeFailureTable {
                page: failure_page.clone(),
                on_select: move |failure: RealtimeFailure| {
                    schema_version.set("1".to_owned());
                    public_payload.set("{}".to_owned());
                    replay_invalid.set(false);
                    selected_failure.set(Some(failure.id));
                    replay_command_id.set(None);
                    notice.set(None);
                },
            }
        },
    };
    let failure_pager = match &failure_state {
        Some(Ok(failure_page)) => rsx! {
            RealtimeFailurePager {
                page: failure_page.clone(),
                page_number: failure_page_number,
            }
        },
        _ => rsx! {},
    };
    let lag_content = match &lag_state {
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
            }
        },
        Some(Ok(lag_page)) => rsx! {
            RealtimeLagTable { page: lag_page.clone() }
            RealtimeLagPager {
                page: lag_page.clone(),
                page_number: lag_page_number,
            }
        },
    };

    rsx! {
        section { class: "admin-realtime-page", aria_labelledby: "outbox-title",
            div { class: "admin-realtime-heading",
                h1 { id: "outbox-title", {page_title} }
                button {
                    r#type: "button",
                    class: "admin-btn-secondary",
                    onclick: move |_| {
                        selected_failure.set(None);
                        replay_command_id.set(None);
                        notice.set(None);
                        event_page_number.set(1);
                        lag_page_number.set(1);
                        failure_page_number.set(1);
                        event_refresh.set(event_refresh().saturating_add(1));
                        lag_refresh.set(lag_refresh().saturating_add(1));
                        failure_refresh.set(failure_refresh().saturating_add(1));
                    },
                    {t!("realtime-refresh")}
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
                    stream_filter: event_stream_filter,
                    kind_filter: event_kind_filter,
                    on_load: load_events,
                }
                {event_content}
                {event_pager}
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
                    div { class: "admin-realtime-status",
                        label { r#for: "outbox-status",
                            {t!("realtime-filter-state")}
                            select {
                                id: "outbox-status",
                                value: failure_resolved_filter(),
                                onchange: move |event| {
                                    let next_state = event.value();
                                    failure_resolved_filter.set(next_state.clone());
                                    active_failure_state_filter.set(next_state);
                                    failure_page_number.set(1);
                                    selected_failure.set(None);
                                    replay_command_id.set(None);
                                    failure_refresh.set(failure_refresh().saturating_add(1));
                                },
                                option { value: "", {t!("realtime-filter-all")} }
                                option { value: "pending", disabled: true, {t!("realtime-state-pending")} }
                                option { value: "retry", disabled: true, {t!("realtime-state-retry")} }
                                option { value: "dead_letter", {t!("realtime-state-dead-letter")} }
                                option { value: "completed", {t!("realtime-state-completed")} }
                            }
                        }
                    }
                }
                {failure_content}
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
                {failure_pager}
                RealtimeFailureFilters {
                    stream_filter: failure_stream_filter,
                    consumer_filter: failure_consumer_filter,
                    code_filter: failure_code_filter,
                    on_load: load_failures,
                }
            }
            if let Some(failure) = selected {
                // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管；.admin-realtime-dialog
                // 内容样式移到面板内的包装 div；共享组件暂不支持 aria_describedby。
                Dialog {
                    title: t!("realtime-replay-title", id: failure.id.clone()),
                    close_label: t!("realtime-replay-cancel"),
                    busy: replay_busy(),
                    on_close: move |_| {
                        selected_failure.set(None);
                        replay_command_id.set(None);
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
                                value: schema_version(),
                                oninput: move |event| {
                                    schema_version.set(event.value());
                                    replay_invalid.set(false);
                                    replay_command_id.set(None);
                                },
                            }
                        }
                        label {
                            {t!("realtime-replay-payload")}
                            textarea {
                                spellcheck: "false",
                                value: public_payload(),
                                oninput: move |event| {
                                    public_payload.set(event.value());
                                    replay_invalid.set(false);
                                    replay_command_id.set(None);
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
                                    if replay_busy() {
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
                                    let command_id = stable_command_id(
                                        replay_command_id,
                                        format!("{failure_id}:{schema}:{payload}"),
                                    );
                                    notice.set(None);
                                    replay_busy.set(true);
                                    let mut replay_busy = replay_busy;
                                    spawn(async move {
                                        let result = replay_realtime_failure(
                                            failure_id,
                                            command_id,
                                            schema,
                                            payload,
                                        )
                                        .await;
                                        replay_busy.set(false);
                                        match result {
                                            Ok(()) => {
                                                notice.set(Some(AdminActionFeedback::Success));
                                                selected_failure.set(None);
                                                replay_command_id.set(None);
                                                event_refresh.set(event_refresh() + 1);
                                                failure_refresh.set(failure_refresh() + 1);
                                                lag_refresh.set(lag_refresh() + 1);
                                            }
                                            Err(code) => notice.set(Some(
                                                AdminActionFeedback::Error(code),
                                            )),
                                        }
                                    });
                                },
                                {t!("realtime-replay-submit")}
                            }
                            button {
                                r#type: "button",
                                class: "admin-btn-secondary",
                                disabled: replay_busy(),
                                onclick: move |_| {
                                    selected_failure.set(None);
                                    replay_command_id.set(None);
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

fn metric_card(label: String, value: String, state: &'static str) -> Element {
    rsx! {
        div { class: "admin-realtime-metric", "data-state": state,
            strong { class: "admin-realtime-metric__value", "{value}" }
            span { class: "admin-realtime-metric__label", "{label}" }
        }
    }
}

fn metric_value(value: Option<i64>) -> String {
    value.map_or_else(|| t!("value-empty"), |value| value.to_string())
}

fn activity_count_state(value: Option<i64>) -> &'static str {
    match value {
        None => "unavailable",
        Some(0) => "neutral",
        Some(_) => "ok",
    }
}

fn consumer_count_state(value: Option<i64>) -> &'static str {
    match value {
        None => "unavailable",
        Some(0) => "warn",
        Some(_) => "ok",
    }
}

fn failure_count_state(value: Option<i64>) -> &'static str {
    match value {
        None => "unavailable",
        Some(0) => "ok",
        Some(_) => "error",
    }
}

fn stable_command_id(mut command_state: Signal<Option<(String, Uuid)>>, signature: String) -> Uuid {
    match command_state() {
        Some((current, command_id)) if current == signature => command_id,
        _ => {
            let command_id = Uuid::new_v4();
            command_state.set(Some((signature, command_id)));
            command_id
        }
    }
}
