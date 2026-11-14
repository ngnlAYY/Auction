//! Realtime failure 表格与分页组件。

use auction_web_shared::TruncatedText;
use auction_web_shared::t;
use auction_web_shared::{AsyncState, AsyncStateKind, Time};
use dioxus::prelude::*;

use crate::api::{RealtimeFailure, RealtimeFailurePage};
use crate::surface::pending::PendingCommands;

/// 展示可供操作员选择 replay 的 failure 安全投影。
#[component]
pub(super) fn RealtimeFailureTable(
    page: RealtimeFailurePage,
    on_select: EventHandler<RealtimeFailure>,
) -> Element {
    let pending = use_context::<PendingCommands>();
    let is_empty = page.failures.is_empty();
    rsx! {
        div { class: "admin-shell__table-wrap",
            table { class: "admin-shell__table admin-realtime-table",
                aria_label: t!("realtime-failure-title"),
                auction_web_shared::TableColumns { layout: "admin-realtime-failures", widths: vec![17, 20, 17, 9, 12, 12, 7, 6] }
                thead {
                    tr {
                        th { scope: "col", class: "admin-realtime-col-failure", {t!("realtime-failure-time")} }
                        th { scope: "col", class: "admin-realtime-col-error", {t!("realtime-failure-error")} }
                        th { scope: "col", class: "admin-realtime-col-subject", {t!("realtime-stream")} }
                        th { scope: "col", class: "admin-realtime-col-topic", {t!("realtime-consumer")} }
                        th { scope: "col", class: "admin-realtime-col-event", {t!("realtime-failure-event-id")} }
                        th { scope: "col", class: "admin-realtime-col-id", {t!("realtime-failure-id")} }
                        th { scope: "col", class: "admin-realtime-col-status", {t!("realtime-failure-state")} }
                        th { scope: "col", {t!("catalog-actions")} }
                    }
                }
                tbody {
                    for failure in page.failures {
                        {
                            let failure_for_action = failure.clone();
                            let error_detail = format!("{}: {}", failure.code, failure.safe_detail);
                            rsx! {
                                // 按业务 ID 固定 key，避免刷新/重排后行内本地状态错位绑定到另一行。
                                tr { key: "{failure.id}", class: "admin-realtime__row",
                                    td { "data-label": t!("realtime-failure-time"),
                                        Time { value: failure.failed_at.clone() }
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
                                    td { "data-label": t!("catalog-actions"),
                                        div { class: "admin-realtime__actions",
                                            if !failure.resolved {
                                                button {
                                                    r#type: "button",
                                                    disabled: pending.any_busy() || pending.contains(&failure.id),
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
        if is_empty {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("realtime-failure-empty"),
                detail: "",
            }
        }
    }
}

fn compact_realtime_id(value: &str) -> String {
    if value.len() <= 12 {
        value.to_owned()
    } else {
        let start = value.char_indices().rev().nth(7).map_or(0, |(index, _)| index);
        value[start..].to_owned()
    }
}
