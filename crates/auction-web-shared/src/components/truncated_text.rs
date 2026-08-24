use dioxus::prelude::*;

/// 展示可能很长的标识或 metadata，并通过浏览器原生提示保留完整值。
///
/// `display` 用于旧版已经提供缩写文本的场景；真正的 `value` 始终放在 `title` 中，避免
/// 截断后丢失审计、UUID 或外部引用的诊断信息。宽度由通用 size 类控制，surface 页面可以
/// 继续通过父表格约束整体布局。
#[component]
pub fn TruncatedText(value: String, display: Option<String>, size: Option<String>) -> Element {
    let display = display.unwrap_or_else(|| value.clone());
    let size = size.unwrap_or_else(|| "medium".to_owned());
    rsx! {
        span {
            class: "shared-truncated-text shared-truncated-text--{size}",
            title: value,
            "{display}"
        }
    }
}
