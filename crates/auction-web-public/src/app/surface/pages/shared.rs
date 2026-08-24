//! Public 页面共用的加载失败状态。

use super::super::*;

/// 公开端错误状态；重试只重新触发当前资源读取，不在浏览器端伪造数据。
#[component]
pub(crate) fn RetryPlaceholder(
    title: String,
    detail: String,
    mut refresh_generation: Signal<u64>,
) -> Element {
    rsx! {
        section {
            class: "public-auction-empty",
            role: "alert",
            aria_labelledby: "retry-placeholder-title",
            h1 { id: "retry-placeholder-title", "{title}" }
            p { "{detail}" }
            button {
                r#type: "button",
                onclick: move |_| refresh_generation.set(refresh_generation().saturating_add(1)),
                {t!("data-retry")}
            }
        }
    }
}
