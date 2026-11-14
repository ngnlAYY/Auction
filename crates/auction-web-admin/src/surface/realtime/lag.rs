//! Realtime lag 表格与分页组件。

use auction_web_shared::TruncatedText;
use auction_web_shared::t;
use auction_web_shared::{AsyncState, AsyncStateKind};
use dioxus::prelude::*;

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
                    aria_label: t!("realtime-lag-title"),
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
