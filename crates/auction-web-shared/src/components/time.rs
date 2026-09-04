//! 本地化时间展示组件。
//!
//! 包装 [`format_localized_time`](crate::i18n::format_localized_time)，统一两端原先手写的
//! `time { datetime: value, "{format_xxx_time(&value)}" }` 模式：机器可读的 `datetime`
//! 属性保留原始 RFC3339 值，可见文本按当前 `<html lang>` 本地化。

use dioxus::prelude::*;

use crate::i18n::format_localized_time;

/// 以 `<time datetime>` 展示 RFC3339 时间。
///
/// `value` 必须是可以被 `format_localized_time` 解析的 RFC3339 时间串；无法解析时按
/// 该函数的既有兜底规则显示「—」。`class` 供调用方叠加状态样式（如倒计时紧迫态）。
#[component]
pub fn Time(value: String, #[props(default)] class: Option<String>) -> Element {
    let formatted = format_localized_time(&value);

    rsx! {
        time { datetime: value, class, "{formatted}" }
    }
}
