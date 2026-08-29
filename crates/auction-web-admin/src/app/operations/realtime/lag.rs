//! Realtime lag 表格与分页组件。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::TruncatedText;

use crate::api::RealtimeLagPage;
use crate::app::surface::pagination::AdminPager;

/// 展示消费者 lag 的安全投影。
#[component]
pub(super) fn RealtimeLagTable(page: RealtimeLagPage) -> Element {
    rsx! {
        if page.lags.is_empty() {
            p { {t!("realtime-lag-empty")} }
        } else {
            div { class: "admin-shell__table-wrap",
                table { class: "admin-shell__table",
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
pub(super) fn RealtimeLagPager(page: RealtimeLagPage, page_number: Signal<i64>) -> Element {
    rsx! {
        AdminPager {
            page: page.pagination.page,
            page_size: page.pagination.page_size,
            total: page.pagination.total,
            label: t!("realtime-lag-title"),
            on_page_change: move |next_page| page_number.set(next_page),
        }
    }
}
