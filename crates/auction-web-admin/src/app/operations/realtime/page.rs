//! Admin realtime 运维页面状态与 replay 对话框。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::{
    RealtimeFailure, fetch_realtime_failure_page, fetch_realtime_lag_page, replay_realtime_failure,
};

use super::failure::{RealtimeFailurePager, RealtimeFailureTable};
use super::filters::{RealtimeFailureFilters, RealtimeLagFilters};
use super::lag::{RealtimeLagPager, RealtimeLagTable};
use crate::app::surface::{AdminLoadError, AdminPanel};

/// Outbox 路由复用已冻结的 realtime failure/lag/replay 安全投影；未冻结的外部广播 sink 不在
/// 页面中虚构状态或成功结果。
pub(crate) fn outbox_page() -> Element {
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
