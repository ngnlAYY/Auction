//! Realtime 事件元数据表格与分页组件。

use auction_web_shared::{TruncatedText, legacy_t as t};
use dioxus::prelude::*;

use crate::api::RealtimeEventPage;
use crate::app::surface::{format_admin_time, pagination::AdminPager};

/// 展示不含 payload 与内部来源字段的最近事件元数据。
#[component]
pub(super) fn RealtimeEventTable(page: RealtimeEventPage) -> Element {
    let is_empty = page.events.is_empty();
    rsx! {
        div { class: "admin-shell__table-wrap",
            table { class: "admin-shell__table admin-realtime-table admin-realtime-event-table",
                thead {
                    tr {
                        th { {t!("realtime-event-created-at")} }
                        th { {t!("realtime-event-kind")} }
                        th { {t!("realtime-stream")} }
                        th { {t!("realtime-event-sequence")} }
                        th { {t!("realtime-event-schema")} }
                        th { {t!("realtime-event-id")} }
                    }
                }
                tbody {
                    for event in page.events {
                        tr { class: "admin-realtime__row",
                            td { "data-label": t!("realtime-event-created-at"),
                                time { datetime: event.created_at.clone(), "{format_admin_time(&event.created_at)}" }
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
            p { class: "admin-realtime-empty", role: "status", {t!("realtime-event-empty")} }
        }
    }
}

/// 控制事件元数据列表分页。
#[component]
pub(super) fn RealtimeEventPager(page: RealtimeEventPage, page_number: Signal<i64>) -> Element {
    rsx! {
        AdminPager {
            page: page.pagination.page,
            page_size: page.pagination.page_size,
            total: page.pagination.total,
            label: t!("realtime-event-title"),
            on_page_change: move |next_page| page_number.set(next_page),
        }
    }
}
