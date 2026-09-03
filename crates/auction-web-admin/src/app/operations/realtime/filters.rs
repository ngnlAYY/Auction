//! Realtime 运维页面的筛选器组件。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

/// 申请 realtime 事件元数据查询筛选条件。
#[component]
pub(super) fn RealtimeEventFilters(
    mut stream_filter: Signal<String>,
    mut kind_filter: Signal<String>,
    on_load: EventHandler<()>,
) -> Element {
    rsx! {
        form {
            class: "admin-realtime-filter admin-realtime-filter--events",
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
                {t!("realtime-filter-kind")}
                input {
                    maxlength: "128",
                    value: kind_filter(),
                    oninput: move |event| kind_filter.set(event.value()),
                }
            }
            button { r#type: "submit", {t!("realtime-filter-apply")} }
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
        details { class: "admin-realtime-disclosure",
            summary { {t!("realtime-failure-filters")} }
            form {
                class: "admin-realtime-filter",
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
