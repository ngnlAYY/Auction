//! Admin realtime 运维页面。
//!
//! 页面展示持久化 lag/failure 安全投影；replay 需要操作员显式提供 schema 版本和公开对象，
//! 不在浏览器端推断或重建事件状态机。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

use auction_web_shared::TruncatedText;

use crate::api::{
    RealtimeFailure, RealtimeFailurePage, RealtimeLagPage, fetch_realtime_failure_page,
    fetch_realtime_lag_page, replay_realtime_failure,
};

use super::pagination::AdminPager;
use super::{AdminLoadError, AdminPanel, format_admin_time};

/// Outbox 路由复用已冻结的 realtime failure/lag/replay 安全投影；未冻结的外部广播 sink 不在
/// 页面中虚构状态或成功结果。
pub(super) fn outbox_page() -> Element {
    page_with_copy(t!("page-outbox-title"), t!("page-outbox-note"))
}

fn page_with_copy(page_title: String, page_note: String) -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let mut notice = use_signal(|| None::<bool>);
    let mut selected_failure = use_signal(|| None::<String>);
    let mut replay_command_id = use_signal(|| None::<(String, Uuid)>);
    let mut schema_version = use_signal(|| 1_i32);
    let mut public_payload = use_signal(|| "{}".to_owned());
    let mut replay_busy = use_signal(|| false);
    let mut lag_page_number = use_signal(|| 1_i64);
    let lag_stream_filter = use_signal(String::new);
    let lag_consumer_filter = use_signal(String::new);
    let mut active_lag_stream_filter = use_signal(String::new);
    let mut active_lag_consumer_filter = use_signal(String::new);
    let mut failure_page_number = use_signal(|| 1_i64);
    let failure_stream_filter = use_signal(String::new);
    let failure_consumer_filter = use_signal(String::new);
    let failure_code_filter = use_signal(String::new);
    let mut failure_resolved_filter = use_signal(String::new);
    let mut active_failure_stream_filter = use_signal(String::new);
    let mut active_failure_consumer_filter = use_signal(String::new);
    let mut active_failure_code_filter = use_signal(String::new);
    let mut active_failure_resolved_filter = use_signal(|| None);
    let lag = use_resource(move || {
        let _ = refresh();
        fetch_realtime_lag_page(
            lag_page_number(),
            active_lag_stream_filter(),
            active_lag_consumer_filter(),
        )
    });
    let failures = use_resource(move || {
        let _ = refresh();
        fetch_realtime_failure_page(
            failure_page_number(),
            active_failure_stream_filter(),
            active_failure_consumer_filter(),
            active_failure_code_filter(),
            active_failure_resolved_filter(),
        )
    });

    let load_lag = move |_| {
        active_lag_stream_filter.set(lag_stream_filter());
        active_lag_consumer_filter.set(lag_consumer_filter());
        lag_page_number.set(1);
        refresh.set(refresh().saturating_add(1));
    };
    let load_failures = move |_| {
        active_failure_stream_filter.set(failure_stream_filter());
        active_failure_consumer_filter.set(failure_consumer_filter());
        active_failure_code_filter.set(failure_code_filter());
        active_failure_resolved_filter.set(match failure_resolved_filter().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        });
        failure_page_number.set(1);
        selected_failure.set(None);
        replay_command_id.set(None);
        refresh.set(refresh().saturating_add(1));
    };
    match (&*lag.read_unchecked(), &*failures.read_unchecked()) {
        (None, _) | (_, None) => rsx! {
            AdminPanel { title: page_title.clone(), detail: t!("realtime-loading") }
        },
        (Some(Err(_)), _) | (_, Some(Err(_))) => rsx! {
            AdminLoadError {
                title: page_title.clone(),
                detail: t!("realtime-error"),
                refresh,
            }
        },
        (Some(Ok(lag_page)), Some(Ok(failure_page))) => {
            let selected_id = selected_failure();
            let selected = selected_id.as_ref().and_then(|id| {
                failure_page.failures.iter().find(|failure| &failure.id == id).cloned()
            });
            rsx! {
                div { class: "admin-realtime-page", aria_labelledby: "realtime-page-title",
                    h1 { id: "realtime-page-title", {page_title} }
                    p { class: "admin-realtime-intro", {page_note} }
                    div { class: "admin-realtime-status",
                    label {
                        {t!("realtime-filter-state")}
                        select {
                            value: failure_resolved_filter(),
                            onchange: move |event| {
                                let next_state = event.value();
                                failure_resolved_filter.set(next_state.clone());
                                active_failure_resolved_filter.set(match next_state.as_str() {
                                    "true" => Some(true),
                                    "false" => Some(false),
                                    _ => None,
                                });
                                failure_page_number.set(1);
                                selected_failure.set(None);
                                replay_command_id.set(None);
                                refresh.set(refresh().saturating_add(1));
                            },
                            option { value: "", {t!("realtime-filter-all")} }
                            option { value: "false", {t!("realtime-filter-unresolved")} }
                            option { value: "true", {t!("realtime-filter-resolved")} }
                        }
                    }
                    }
                    RealtimeLagFilters {
                        stream_filter: lag_stream_filter,
                        consumer_filter: lag_consumer_filter,
                        on_load: load_lag,
                    }
                    RealtimeLagTable { page: lag_page.clone() }
                    RealtimeLagPager {
                        page: lag_page.clone(),
                        page_number: lag_page_number,
                    }
                    RealtimeFailureFilters {
                        stream_filter: failure_stream_filter,
                        consumer_filter: failure_consumer_filter,
                        code_filter: failure_code_filter,
                        on_load: load_failures,
                    }
                    RealtimeFailureTable {
                        page: failure_page.clone(),
                        on_select: move |failure: RealtimeFailure| {
                            schema_version.set(1);
                            public_payload.set("{}".to_owned());
                            selected_failure.set(Some(failure.id));
                            replay_command_id.set(None);
                            notice.set(None);
                        },
                    }
                    RealtimeFailurePager {
                        page: failure_page.clone(),
                        page_number: failure_page_number,
                    }
                    if let Some(failure) = selected {
                        div { class: "admin-settlement__dialog-backdrop", role: "presentation",
                            section {
                                class: "admin-settlement__dialog admin-realtime-dialog",
                                role: "dialog",
                                aria_modal: "true",
                                aria_labelledby: "realtime-replay-title",
                                aria_busy: replay_busy().to_string(),
                                tabindex: "-1",
                                onkeydown: move |event| {
                                    if event.key() == Key::Escape && !replay_busy() {
                                        selected_failure.set(None);
                                        replay_command_id.set(None);
                                    }
                                },
                                h2 { id: "realtime-replay-title", {t!("realtime-replay-title")} }
                                p { class: "admin-panel__note", "{failure.id}" }
                                label {
                                    {t!("realtime-replay-schema")}
                                    input {
                                        r#type: "number",
                                        min: "1",
                                        disabled: replay_busy(),
                                        value: schema_version().to_string(),
                                        oninput: move |event| {
                                            if let Ok(value) = event.value().parse::<i32>() {
                                                schema_version.set(value);
                                                replay_command_id.set(None);
                                            }
                                        },
                                    }
                                }
                                label {
                                    {t!("realtime-replay-payload")}
                                    textarea {
                                        disabled: replay_busy(),
                                        value: public_payload(),
                                        oninput: move |event| {
                                            public_payload.set(event.value());
                                            replay_command_id.set(None);
                                        },
                                    }
                                }
                                div { class: "admin-panel__actions",
                                    button {
                                        r#type: "button",
                                        disabled: replay_busy(),
                                        onclick: move |_| {
                                            if replay_busy() {
                                                return;
                                            }
                                            let failure_id = failure.id.clone();
                                            let schema = schema_version();
                                            let payload = public_payload();
                                            let command_id = stable_command_id(
                                                replay_command_id,
                                                format!("{failure_id}:{schema}:{payload}"),
                                            );
                                            notice.set(None);
                                            replay_busy.set(true);
                                            let mut replay_busy = replay_busy;
                                            spawn(async move {
                                                let successful = replay_realtime_failure(
                                                    failure_id,
                                                    command_id,
                                                    schema,
                                                    payload,
                                                )
                                                .await
                                                .is_ok();
                                                replay_busy.set(false);
                                                notice.set(Some(successful));
                                                if successful {
                                                    selected_failure.set(None);
                                                    replay_command_id.set(None);
                                                    refresh.set(refresh() + 1);
                                                }
                                            });
                                        },
                                        if replay_busy() { {t!("admin-action-working")} } else { {t!("realtime-replay-submit")} }
                                    }
                                    button {
                                        r#type: "button",
                                        class: "admin-btn-secondary",
                                        disabled: replay_busy(),
                                        autofocus: true,
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
                    if let Some(successful) = notice() {
                        p { class: "admin-panel__note action-feedback", role: "status",
                            if successful { {t!("realtime-replay-success")} } else { {t!("realtime-replay-error")} }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn RealtimeLagFilters(
    mut stream_filter: Signal<String>,
    mut consumer_filter: Signal<String>,
    on_load: EventHandler<()>,
) -> Element {
    rsx! {
        details { class: "admin-realtime-advanced",
            summary { {t!("realtime-lag-filters")} }
            form {
                class: "admin-panel__actions admin-realtime-filters",
                onsubmit: move |event| {
                    event.prevent_default();
                    on_load.call(());
                },
                label {
                    {t!("realtime-filter-stream")}
                    input {
                        maxlength: "256",
                        value: stream_filter(),
                        oninput: move |event| stream_filter.set(event.value()),
                    }
                }
                label {
                    {t!("realtime-filter-consumer")}
                    input {
                        maxlength: "128",
                        value: consumer_filter(),
                        oninput: move |event| consumer_filter.set(event.value()),
                    }
                }
                button { r#type: "submit", {t!("realtime-filter-apply")} }
            }
        }
    }
}

#[component]
fn RealtimeFailureFilters(
    mut stream_filter: Signal<String>,
    mut consumer_filter: Signal<String>,
    mut code_filter: Signal<String>,
    on_load: EventHandler<()>,
) -> Element {
    rsx! {
        details { class: "admin-realtime-advanced",
            summary { {t!("realtime-failure-filters")} }
            form {
                class: "admin-panel__actions admin-realtime-filters",
                onsubmit: move |event| {
                    event.prevent_default();
                    on_load.call(());
                },
                label {
                    {t!("realtime-filter-stream")}
                    input {
                        maxlength: "256",
                        value: stream_filter(),
                        oninput: move |event| stream_filter.set(event.value()),
                    }
                }
                label {
                    {t!("realtime-filter-consumer")}
                    input {
                        maxlength: "128",
                        value: consumer_filter(),
                        oninput: move |event| consumer_filter.set(event.value()),
                    }
                }
                label {
                    {t!("realtime-filter-code")}
                    input {
                        maxlength: "64",
                        value: code_filter(),
                        oninput: move |event| code_filter.set(event.value()),
                    }
                }
                button { r#type: "submit", {t!("realtime-filter-apply")} }
            }
        }
    }
}

#[component]
fn RealtimeLagTable(page: RealtimeLagPage) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "realtime-lag-title",
            h2 { id: "realtime-lag-title", {t!("realtime-lag-title")} }
            p { class: "admin-panel__note", {t!("realtime-lag-total", total: page.total)} }
            if page.lags.is_empty() {
                p { class: "admin-panel__note", {t!("realtime-lag-empty")} }
            } else {
                div { class: "public-auction-catalog admin-realtime__table-wrap",
                    table {
                        thead {
                            tr {
                                th { {t!("realtime-stream")} }
                                th { {t!("realtime-consumer")} }
                                th { {t!("realtime-offset")} }
                                th { {t!("realtime-latest-sequence")} }
                                th { {t!("realtime-lag")} }
                            }
                        }
                        tbody {
                            for lag in page.lags {
                                tr { class: "admin-realtime__row",
                                    td { "data-label": t!("realtime-stream"), TruncatedText { value: lag.stream_key } }
                                    td { "data-label": t!("realtime-consumer"), TruncatedText { value: lag.consumer_key } }
                                    td {
                                        "data-label": t!("realtime-offset"),
                                        class: "public-auction-catalog__number",
                                        "{lag.offset}"
                                    }
                                    td {
                                        "data-label": t!("realtime-latest-sequence"),
                                        class: "public-auction-catalog__number",
                                        "{lag.latest_sequence}"
                                    }
                                    td {
                                        "data-label": t!("realtime-lag"),
                                        class: "public-auction-catalog__number",
                                        "{lag.lag}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn RealtimeLagPager(page: RealtimeLagPage, page_number: Signal<i64>) -> Element {
    rsx! {
        AdminPager {
            page: page.page,
            page_size: page.page_size,
            total: page.total,
            label: t!("realtime-pagination"),
            on_page_change: move |next_page| page_number.set(next_page),
        }
    }
}

#[component]
fn RealtimeFailureTable(
    page: RealtimeFailurePage,
    on_select: EventHandler<RealtimeFailure>,
) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "realtime-failure-title",
            h2 { id: "realtime-failure-title", {t!("realtime-failure-title")} }
            p { class: "admin-panel__note", {t!("realtime-failure-total", total: page.total)} }
            if page.failures.is_empty() {
                p { class: "admin-panel__note", {t!("realtime-failure-empty")} }
            } else {
                div { class: "public-auction-catalog admin-realtime__table-wrap",
                    table {
                        thead {
                            tr {
                                th { {t!("realtime-failure-id")} }
                                th { {t!("realtime-failure-event-id")} }
                                th { {t!("realtime-failure-code")} }
                                th { {t!("realtime-failure-stream")} }
                                th { {t!("realtime-failure-consumer")} }
                                th { {t!("realtime-failure-detail")} }
                                th { {t!("realtime-failure-time")} }
                                th { {t!("realtime-failure-state")} }
                                th { {t!("catalog-actions")} }
                            }
                        }
                        tbody {
                            for failure in page.failures {
                                {
                                    let failure_for_action = failure.clone();
                                    rsx! {
                                        tr { class: "admin-realtime__row",
                                            td { "data-label": t!("realtime-failure-id"),
                                                TruncatedText {
                                                    display: compact_realtime_id(&failure.id),
                                                    size: "compact",
                                                    value: failure.id.clone()
                                                }
                                            }
                                            td { "data-label": t!("realtime-failure-event-id"),
                                                TruncatedText {
                                                    display: compact_realtime_id(&failure.event_id),
                                                    size: "compact",
                                                    value: failure.event_id.clone()
                                                }
                                            }
                                            td { "data-label": t!("realtime-failure-code"), TruncatedText { value: failure.code.clone() } }
                                            td { "data-label": t!("realtime-failure-stream"), TruncatedText { value: failure.stream_key.clone() } }
                                            td { "data-label": t!("realtime-failure-consumer"), TruncatedText { value: failure.consumer_key.clone() } }
                                            td { "data-label": t!("realtime-failure-detail"), TruncatedText { value: failure.safe_detail.clone(), size: "wide" } }
                                            td { "data-label": t!("realtime-failure-time"),
                                                time { datetime: failure.failed_at.clone(), "{format_admin_time(&failure.failed_at)}" }
                                            }
                                            td { "data-label": t!("realtime-failure-state"),
                                                span {
                                                    class: if failure.resolved {
                                                        "admin-realtime__status admin-realtime__status--resolved"
                                                    } else {
                                                        "admin-realtime__status admin-realtime__status--unresolved"
                                                    },
                                                    if failure.resolved {
                                                        {t!("realtime-resolved")}
                                                    } else {
                                                        {t!("realtime-unresolved")}
                                                    }
                                                }
                                            }
                                            td { "data-label": t!("catalog-actions"), class: "admin-realtime__actions",
                                                button {
                                                    r#type: "button",
                                                    disabled: failure.resolved,
                                                    onclick: move |_| on_select.call(failure_for_action.clone()),
                                                    {t!("realtime-replay")}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn compact_realtime_id(value: &str) -> String {
    if value.len() <= 12 {
        value.to_owned()
    } else {
        value.chars().rev().take(8).collect::<String>().chars().rev().collect()
    }
}

#[component]
fn RealtimeFailurePager(page: RealtimeFailurePage, page_number: Signal<i64>) -> Element {
    rsx! {
        AdminPager {
            page: page.page,
            page_size: page.page_size,
            total: page.total,
            label: t!("realtime-pagination"),
            on_page_change: move |next_page| page_number.set(next_page),
        }
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
