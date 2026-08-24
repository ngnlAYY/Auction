//! 管理端跨页面展示辅助组件。

use dioxus::prelude::*;
use dioxus_i18n::t;

use auction_web_shared::format_localized_time;

/// 按旧版管理端约定把 UTC 时间显示为当前界面语言的 Asia/Shanghai 时间。
///
/// 旧版使用浏览器 `Intl.DateTimeFormat`，因此不能在 Rust 侧固定月份语言；WASM 路径继续
/// 使用浏览器的本地化格式，非 WASM 构建只保留静态检查所需的稳定兜底。`datetime` 属性
/// 仍保留原始 RFC3339 值，屏幕文案统一经过这一层。
pub fn format_admin_time(value: &str) -> String {
    format_localized_time(value)
}

/// 管理端异步状态面板。
///
/// 页面在加载或等待权威数据时复用该结构；它不是未实现功能的静态占位页。
#[component]
pub fn AdminPanel(title: String, detail: String) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-panel-title",
            h1 { id: "admin-panel-title", "{title}" }
            p { class: "admin-panel__note", "{detail}" }
        }
    }
}

/// 管理端首屏数据加载失败时提供统一的重试入口。
///
/// 各页面的资源都由自己的刷新信号驱动；这里不保存错误正文，也不把底层响应暴露给浏览器。
#[component]
pub fn AdminLoadError(title: String, detail: String, mut refresh: Signal<u64>) -> Element {
    rsx! {
        section {
            class: "admin-panel",
            role: "alert",
            aria_labelledby: "admin-load-error-title",
            h1 { id: "admin-load-error-title", "{title}" }
            p { class: "admin-panel__note", "{detail}" }
            button {
                r#type: "button",
                onclick: move |_| refresh.set(refresh().saturating_add(1)),
                {t!("admin-retry")}
            }
        }
    }
}
