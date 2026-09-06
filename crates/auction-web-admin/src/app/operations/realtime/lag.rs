//! Realtime lag 表格与分页组件。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::TruncatedText;

use auction_web_shared::{AsyncState, AsyncStateKind, Pager};

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
                    // 列序：消费者、流、偏移、最新序号、延迟；按字段预期内容分配比例。
                    auction_web_shared::TableColumns { layout: "admin-realtime-lag", widths: vec![22, 45, 11, 12, 10] }
                    thead {
                        tr {
                            th { {t!("realtime-consumer")} }
                            th { {t!("realtime-stream")} }
                            th { {t!("realtime-offset")} }
                            th { {t!("realtime-latest-sequence")} }
                            th { {t!("realtime-lag")} }
                        }
                    }
                    tbody {
                        for lag in page.lags {
                            tr { class: "admin-realtime__row",
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
    page: RealtimeLagPage,
    on_page_change: EventHandler<i64>,
) -> Element {
    rsx! {
        Pager {
            page: page.pagination.page,
            page_size: page.pagination.page_size,
            total: Some(page.pagination.total),
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
