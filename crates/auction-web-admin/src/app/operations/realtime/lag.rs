//! Realtime lag 表格与分页组件。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::TruncatedText;

use crate::app::surface::pagination::{AdminPager as Pager, AdminPagination};
use auction_web_shared::{AsyncState, AsyncStateKind};

use crate::api::RealtimeLagPage;

/// 展示消费者 lag 的安全投影。
#[component]
pub(super) fn RealtimeLagTable(page: RealtimeLagPage) -> Element {
    rsx! {
        if page.lags.is_empty() {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("realtime-lag-empty"),
                detail: "",
            }
        } else {
            div { class: "admin-shell__table-wrap",
                table { class: "admin-shell__table admin-realtime-table",
                    caption { class: "admin-visually-hidden", {t!("realtime-lag-title")} }
                    auction_web_shared::TableColumns { layout: "admin-realtime-lag", widths: vec![22, 45, 11, 12, 10] }
                    thead {
                        tr {
                            th { scope: "col", {t!("realtime-consumer")} }
                            th { scope: "col", {t!("realtime-stream")} }
                            th { scope: "col", {t!("realtime-offset")} }
                            th { scope: "col", {t!("realtime-latest-sequence")} }
                            th { scope: "col", {t!("realtime-lag")} }
                        }
                    }
                    tbody {
                        for lag in page.lags {
                            tr { key: "{lag.consumer_key.len()}:{lag.consumer_key}{lag.stream_key}", class: "admin-realtime__row",
                                td { "data-label": t!("realtime-consumer"), TruncatedText { value: lag.consumer_key } }
                                td { "data-label": t!("realtime-stream"), TruncatedText { value: lag.stream_key } }
                                td {
                                    "data-label": t!("realtime-offset"),
                                    "{lag.offset}"
                                }
                                td {
                                    "data-label": t!("realtime-latest-sequence"),
                                    "{lag.latest_sequence}"
                                }
                                td {
                                    "data-label": t!("realtime-lag"),
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

/// 控制 lag 列表分页并把页码交回页面状态。
#[component]
pub(super) fn RealtimeLagPager(
    page: i64,
    page_size: i64,
    total: i64,
    pagination: AdminPagination,
    on_page_change: EventHandler<i64>,
) -> Element {
    rsx! {
        Pager {
            pagination,
            page,
            page_size,
            total: Some(total),
            label: t!("realtime-lag-title"),
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
