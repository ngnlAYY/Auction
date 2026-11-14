//! Realtime 事件元数据表格与分页组件。

use auction_web_shared::{TruncatedText, t};
use dioxus::prelude::*;

use auction_web_shared::{AsyncState, AsyncStateKind, Time};

use crate::api::RealtimeEventPage;

/// 展示不含 payload 与内部来源字段的最近事件元数据。
#[component]
pub(super) fn RealtimeEventTable(page: RealtimeEventPage) -> Element {
    let is_empty = page.events.is_empty();
    rsx! {
        div { class: "admin-shell__table-wrap",
            table { class: "admin-shell__table admin-realtime-table admin-realtime-event-table",
                aria_label: t!("realtime-event-title"),
                auction_web_shared::TableColumns { layout: "admin-realtime-events", widths: vec![18, 22, 26, 6, 7, 21] }
                thead {
                    tr {
                        th { scope: "col", {t!("realtime-event-created-at")} }
                        th { scope: "col", {t!("realtime-event-kind")} }
                        th { scope: "col", {t!("realtime-stream")} }
                        th { scope: "col", {t!("realtime-event-sequence")} }
                        th { scope: "col", {t!("realtime-event-schema")} }
                        th { scope: "col", {t!("realtime-event-id")} }
                    }
                }
                tbody {
                    for event in page.events {
                        tr { key: "{event.event_id}", class: "admin-realtime__row",
                            td { "data-label": t!("realtime-event-created-at"),
                                Time { value: event.created_at.clone() }
                            }
                            td { "data-label": t!("realtime-event-kind"),
                                TruncatedText { value: event.kind }
                            }
                            td { "data-label": t!("realtime-stream"),
                                TruncatedText { value: event.stream_key }
                            }
                            td {
                                "data-label": t!("realtime-event-sequence"),
                                class: "admin-realtime__number",
                                "{event.stream_sequence}"
                            }
                            td {
                                "data-label": t!("realtime-event-schema"),
                                class: "admin-realtime__number",
                                "v{event.schema_version}"
                            }
                            td { "data-label": t!("realtime-event-id"),
                                TruncatedText { value: event.event_id, size: "compact" }
                            }
                        }
                    }
                }
            }
        }
        if is_empty {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("realtime-event-empty"),
                detail: "",
            }
        }
    }
}
