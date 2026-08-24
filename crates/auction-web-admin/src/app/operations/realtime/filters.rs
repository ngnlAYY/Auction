//! Realtime 运维页面的筛选器组件。

use dioxus::prelude::*;
use dioxus_i18n::t;

/// 申请 lag 查询筛选条件。
#[component]
pub(super) fn RealtimeLagFilters(
    mut stream_filter: Signal<String>,
    mut consumer_filter: Signal<String>,
    on_load: EventHandler<()>,
) -> Element {
    rsx! {
        details { class: "admin-realtime-advanced",
            summary { {t!("realtime-lag-filters")} }
            form {
                class: "admin-panel__actions admin-realtime-filters",
                onsubmit: move |event| {
                    event.prevent_default();
                    on_load.call(());
                },
                label {
                    {t!("realtime-filter-stream")}
                    input {
                        maxlength: "256",
                        value: stream_filter(),
                        oninput: move |event| stream_filter.set(event.value()),
                    }
                }
                label {
                    {t!("realtime-filter-consumer")}
                    input {
                        maxlength: "128",
                        value: consumer_filter(),
                        oninput: move |event| consumer_filter.set(event.value()),
                    }
                }
                button { r#type: "submit", {t!("realtime-filter-apply")} }
            }
        }
    }
}

/// 申请 realtime failure 查询筛选条件。
#[component]
pub(super) fn RealtimeFailureFilters(
    mut stream_filter: Signal<String>,
    mut consumer_filter: Signal<String>,
    mut code_filter: Signal<String>,
    on_load: EventHandler<()>,
) -> Element {
    rsx! {
        details { class: "admin-realtime-advanced",
            summary { {t!("realtime-failure-filters")} }
            form {
                class: "admin-panel__actions admin-realtime-filters",
                onsubmit: move |event| {
                    event.prevent_default();
                    on_load.call(());
                },
                label {
                    {t!("realtime-filter-stream")}
                    input {
                        maxlength: "256",
                        value: stream_filter(),
                        oninput: move |event| stream_filter.set(event.value()),
                    }
                }
                label {
                    {t!("realtime-filter-consumer")}
                    input {
                        maxlength: "128",
                        value: consumer_filter(),
                        oninput: move |event| consumer_filter.set(event.value()),
                    }
                }
                label {
                    {t!("realtime-filter-code")}
                    input {
                        maxlength: "64",
                        value: code_filter(),
                        oninput: move |event| code_filter.set(event.value()),
                    }
                }
                button { r#type: "submit", {t!("realtime-filter-apply")} }
            }
        }
    }
}
