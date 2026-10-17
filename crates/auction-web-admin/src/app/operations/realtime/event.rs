//! Realtime 事件元数据表格与分页组件。

use auction_web_shared::{TruncatedText, legacy_t as t};
use dioxus::prelude::*;

use crate::app::surface::pagination::{AdminPager as Pager, AdminPagination};
use auction_web_shared::{AsyncState, AsyncStateKind, Time};

use crate::api::RealtimeEventPage;

/// 展示不含 payload 与内部来源字段的最近事件元数据。
#[component]
pub(super) fn RealtimeEventTable(page: RealtimeEventPage) -> Element {
    let is_empty = page.events.is_empty();
    rsx! {
        div { class: "admin-shell__table-wrap",
            table { class: "admin-shell__table admin-realtime-table admin-realtime-event-table",
                // 列序：时间、类型、流、序号、Schema、事件 ID；按字段预期内容分配比例。
                auction_web_shared::TableColumns { layout: "admin-realtime-events", widths: vec![18, 22, 26, 6, 7, 21] }
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

/// 控制事件元数据列表分页。
#[component]
pub(super) fn RealtimeEventPager(
    page: RealtimeEventPage,
    pagination: AdminPagination,
    on_page_change: EventHandler<i64>,
) -> Element {
    rsx! {
        Pager {
            pagination,
            page: page.pagination.page,
            page_size: page.pagination.page_size,
            total: Some(page.pagination.total),
            label: t!("realtime-event-title"),
            previous_label: t!("admin-pagination-previous"),
            status: move |(page, pages): (i64, i64)| {
                t!("admin-pagination-status", page: page, pages: pages)
            },
            next_label: t!("admin-pagination-next"),
            jump_label: t!("admin-pagination-input"),
            jump_button_label: t!("admin-pagination-go"),
            on_page_change,
        }
    }
}
