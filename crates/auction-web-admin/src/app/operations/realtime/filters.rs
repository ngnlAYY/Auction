//! Realtime 运维页面的筛选器组件。
//! 筛选草稿是各筛选组件的本地信号：键击只重渲染本表单，不再把草稿信号挂在页面顶层
//! 导致每次输入触发整页重渲染；提交时才把 trim 后的已应用值回传给页面写回 URL。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

/// 申请 realtime 事件元数据查询筛选条件。
#[component]
pub(super) fn RealtimeEventFilters(
    /// 草稿初值（地址栏恢复的已应用筛选）；仅在挂载时取一次，之后的编辑不落 URL。
    initial_stream: String,
    initial_kind: String,
    on_load: EventHandler<(String, String)>,
) -> Element {
    let mut stream_filter = use_signal(|| initial_stream.clone());
    let mut kind_filter = use_signal(|| initial_kind.clone());
    rsx! {
        form {
            class: "admin-realtime-filter admin-realtime-filter--events",
            onsubmit: move |event| {
                event.prevent_default();
                on_load
                    .call((
                        stream_filter().trim().to_owned(),
                        kind_filter().trim().to_owned(),
                    ));
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
    initial_stream: String,
    initial_consumer: String,
    initial_code: String,
    on_load: EventHandler<(String, String, String)>,
) -> Element {
    let mut stream_filter = use_signal(|| initial_stream.clone());
    let mut consumer_filter = use_signal(|| initial_consumer.clone());
    let mut code_filter = use_signal(|| initial_code.clone());
    rsx! {
        details { class: "admin-realtime-disclosure",
            summary { {t!("realtime-failure-filters")} }
            form {
                class: "admin-realtime-filter",
                onsubmit: move |event| {
                    event.prevent_default();
                    on_load
                        .call((
                            stream_filter().trim().to_owned(),
                            consumer_filter().trim().to_owned(),
                            code_filter().trim().to_owned(),
                        ));
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
