//! Realtime failure 表格与分页组件。

use dioxus::prelude::*;
use dioxus_i18n::t;

use auction_web_shared::TruncatedText;

use crate::api::{RealtimeFailure, RealtimeFailurePage};
use crate::app::surface::{format_admin_time, pagination::AdminPager};

/// 展示可供操作员选择 replay 的 failure 安全投影。
#[component]
pub(super) fn RealtimeFailureTable(
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

/// 控制 failure 列表分页。
#[component]
pub(super) fn RealtimeFailurePager(page: RealtimeFailurePage, page_number: Signal<i64>) -> Element {
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
