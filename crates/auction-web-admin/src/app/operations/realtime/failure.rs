//! Realtime failure 表格与分页组件。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::TruncatedText;

use crate::api::{RealtimeFailure, RealtimeFailurePage};
use crate::app::surface::{format_admin_time, pagination::AdminPager};

/// 展示可供操作员选择 replay 的 failure 安全投影。
#[component]
pub(super) fn RealtimeFailureTable(
    page: RealtimeFailurePage,
    on_select: EventHandler<RealtimeFailure>,
) -> Element {
    let is_empty = page.failures.is_empty();
    rsx! {
        div { class: "admin-shell__table-wrap",
            table { class: "admin-shell__table admin-realtime-table",
                thead {
                    tr {
                        th { class: "admin-realtime-col-failure", {t!("realtime-failure-time")} }
                        th { class: "admin-realtime-col-error", {t!("realtime-failure-error")} }
                        th { class: "admin-realtime-col-subject", {t!("realtime-stream")} }
                        th { class: "admin-realtime-col-topic", {t!("realtime-consumer")} }
                        th { class: "admin-realtime-col-event", {t!("realtime-failure-event-id")} }
                        th { class: "admin-realtime-col-id", {t!("realtime-failure-id")} }
                        th { class: "admin-realtime-col-status", {t!("realtime-failure-state")} }
                        th { {t!("catalog-actions")} }
                    }
                }
                tbody {
                    for failure in page.failures {
                        {
                            let failure_for_action = failure.clone();
                            let error_detail = format!("{}: {}", failure.code, failure.safe_detail);
                            rsx! {
                                tr { class: "admin-realtime__row",
                                    td { "data-label": t!("realtime-failure-time"),
                                        time { datetime: failure.failed_at.clone(), "{format_admin_time(&failure.failed_at)}" }
                                    }
                                    td { "data-label": t!("realtime-failure-error"), class: "admin-realtime__error",
                                        TruncatedText {
                                            display: failure.code.clone(),
                                            size: "wide",
                                            value: error_detail,
                                        }
                                    }
                                    td { "data-label": t!("realtime-stream"), TruncatedText { value: failure.stream_key.clone() } }
                                    td { "data-label": t!("realtime-consumer"), TruncatedText { value: failure.consumer_key.clone() } }
                                    td { "data-label": t!("realtime-failure-event-id"),
                                        TruncatedText {
                                            display: compact_realtime_id(&failure.event_id),
                                            size: "compact",
                                            value: failure.event_id.clone()
                                        }
                                    }
                                    td { "data-label": t!("realtime-failure-id"),
                                        TruncatedText {
                                            display: compact_realtime_id(&failure.id),
                                            size: "compact",
                                            value: failure.id.clone()
                                        }
                                    }
                                    td { "data-label": t!("realtime-failure-state"),
                                        span {
                                            class: if failure.resolved {
                                                "admin-realtime__status admin-realtime__status--resolved"
                                            } else {
                                                "admin-realtime__status admin-realtime__status--unresolved"
                                            },
                                            if failure.resolved {
                                                {t!("realtime-state-completed")}
                                            } else {
                                                {t!("realtime-state-dead-letter")}
                                            }
                                        }
                                    }
                                    td { "data-label": t!("catalog-actions"), class: "admin-realtime__actions",
                                        if !failure.resolved {
                                            button {
                                                r#type: "button",
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
        if is_empty {
            p { class: "admin-realtime-empty", role: "status", {t!("realtime-failure-empty")} }
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

/// 控制 failure 列表分页。
#[component]
pub(super) fn RealtimeFailurePager(page: RealtimeFailurePage, page_number: Signal<i64>) -> Element {
    rsx! {
        AdminPager {
            page: page.pagination.page,
            page_size: page.pagination.page_size,
            total: page.pagination.total,
            label: t!("realtime-failure-title"),
            on_page_change: move |next_page| page_number.set(next_page),
        }
    }
}
