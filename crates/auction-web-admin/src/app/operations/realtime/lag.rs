//! Realtime lag 表格与分页组件。

use dioxus::prelude::*;
use dioxus_i18n::t;

use auction_web_shared::TruncatedText;

use crate::api::RealtimeLagPage;
use crate::app::surface::pagination::AdminPager;

/// 展示消费者 lag 的安全投影。
#[component]
pub(super) fn RealtimeLagTable(page: RealtimeLagPage) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "realtime-lag-title",
            h2 { id: "realtime-lag-title", {t!("realtime-lag-title")} }
            p { class: "admin-panel__note", {t!("realtime-lag-total", total: page.total)} }
            if page.lags.is_empty() {
                p { class: "admin-panel__note", {t!("realtime-lag-empty")} }
            } else {
                div { class: "public-auction-catalog admin-realtime__table-wrap",
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
                                tr { class: "admin-realtime__row",
                                    td { "data-label": t!("realtime-stream"), TruncatedText { value: lag.stream_key } }
                                    td { "data-label": t!("realtime-consumer"), TruncatedText { value: lag.consumer_key } }
                                    td {
                                        "data-label": t!("realtime-offset"),
                                        class: "public-auction-catalog__number",
                                        "{lag.offset}"
                                    }
                                    td {
                                        "data-label": t!("realtime-latest-sequence"),
                                        class: "public-auction-catalog__number",
                                        "{lag.latest_sequence}"
                                    }
                                    td {
                                        "data-label": t!("realtime-lag"),
                                        class: "public-auction-catalog__number",
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
}

/// 控制 lag 列表分页并把页码交回页面状态。
#[component]
pub(super) fn RealtimeLagPager(page: RealtimeLagPage, page_number: Signal<i64>) -> Element {
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
