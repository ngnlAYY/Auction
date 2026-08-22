//! Admin realtime 运维页面。
//!
//! 页面展示持久化 lag/failure 安全投影；replay 需要操作员显式提供 schema 版本和公开对象，
//! 不在浏览器端推断或重建事件状态机。

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::{
    RealtimeFailure, RealtimeFailurePage, RealtimeLagPage, fetch_realtime_failures,
    fetch_realtime_lag, replay_realtime_failure,
};

use super::AdminPanel;

pub(super) fn page() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let mut notice = use_signal(|| None::<bool>);
    let mut selected_failure = use_signal(|| None::<String>);
    let mut replay_command_id = use_signal(|| None::<Uuid>);
    let mut schema_version = use_signal(|| 1_i32);
    let mut public_payload = use_signal(|| "{}".to_owned());
    let lag = use_resource(move || {
        let _ = refresh();
        fetch_realtime_lag()
    });
    let failures = use_resource(move || {
        let _ = refresh();
        fetch_realtime_failures()
    });

    match (&*lag.read_unchecked(), &*failures.read_unchecked()) {
        (None, _) | (_, None) => rsx! {
            AdminPanel { title: t!("page-status-title"), detail: t!("realtime-loading") }
        },
        (Some(Err(_)), _) | (_, Some(Err(_))) => rsx! {
            AdminPanel { title: t!("page-status-title"), detail: t!("realtime-error") }
        },
        (Some(Ok(lag_page)), Some(Ok(failure_page))) => {
            let selected_id = selected_failure();
            let selected = selected_id.as_ref().and_then(|id| {
                failure_page.failures.iter().find(|failure| &failure.id == id).cloned()
            });
            rsx! {
                RealtimeLagTable { page: lag_page.clone() }
                RealtimeFailureTable {
                    page: failure_page.clone(),
                    on_select: move |failure: RealtimeFailure| {
                        schema_version.set(1);
                        public_payload.set("{}".to_owned());
                        selected_failure.set(Some(failure.id));
                        replay_command_id.set(Some(Uuid::new_v4()));
                        notice.set(None);
                    },
                }
                if let Some(failure) = selected {
                    section { class: "admin-panel", aria_labelledby: "realtime-replay-title",
                        h2 { id: "realtime-replay-title", {t!("realtime-replay-title")} }
                        p { class: "admin-panel__note", "{failure.id}" }
                        label {
                            {t!("realtime-replay-schema")}
                            input {
                                r#type: "number",
                                min: "1",
                                value: schema_version().to_string(),
                                oninput: move |event| {
                                    if let Ok(value) = event.value().parse::<i32>() {
                                        schema_version.set(value);
                                    }
                                },
                            }
                        }
                        label {
                            {t!("realtime-replay-payload")}
                            textarea {
                                value: public_payload(),
                                oninput: move |event| public_payload.set(event.value()),
                            }
                        }
                        div { class: "admin-panel__actions",
                            button {
                                r#type: "button",
                                onclick: move |_| {
                                    let failure_id = failure.id.clone();
                                    let Some(command_id) = replay_command_id() else {
                                        return;
                                    };
                                    let schema = schema_version();
                                    let payload = public_payload();
                                    notice.set(None);
                                    spawn(async move {
                                        let successful = replay_realtime_failure(
                                            failure_id,
                                            command_id,
                                            schema,
                                            payload,
                                        )
                                        .await
                                        .is_ok();
                                        notice.set(Some(successful));
                                        if successful {
                                            selected_failure.set(None);
                                            replay_command_id.set(None);
                                            refresh.set(refresh() + 1);
                                        }
                                    });
                                },
                                {t!("realtime-replay-submit")}
                            }
                            button {
                                r#type: "button",
                                onclick: move |_| {
                                    selected_failure.set(None);
                                    replay_command_id.set(None);
                                },
                                {t!("realtime-replay-cancel")}
                            }
                        }
                    }
                }
                if let Some(successful) = notice() {
                    p { class: "admin-panel__note", role: "status",
                        if successful { {t!("realtime-replay-success")} } else { {t!("realtime-replay-error")} }
                    }
                }
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
            div { class: "public-auction-catalog",
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
                            tr {
                                td { "{lag.stream_key}" }
                                td { "{lag.consumer_key}" }
                                td { class: "public-auction-catalog__number", "{lag.offset}" }
                                td { class: "public-auction-catalog__number", "{lag.latest_sequence}" }
                                td { class: "public-auction-catalog__number", "{lag.lag}" }
                            }
                        }
                    }
                }
            }
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
            div { class: "public-auction-catalog",
                table {
                    thead {
                        tr {
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
                            tr {
                                td { "{failure.code}" }
                                td { "{failure.stream_key}" }
                                td { "{failure.consumer_key}" }
                                td { "{failure.safe_detail}" }
                                td { "{failure.failed_at}" }
                                td {
                                    if failure.resolved {
                                        {t!("realtime-resolved")}
                                    } else {
                                        {t!("realtime-unresolved")}
                                    }
                                }
                                td {
                                    button {
                                        r#type: "button",
                                        onclick: move |_| on_select.call(failure.clone()),
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
