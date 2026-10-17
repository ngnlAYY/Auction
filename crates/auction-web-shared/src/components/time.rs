//! 本地化时间展示组件。
//! 机器可读的 `datetime` 保留原始 RFC3339 值，可见文本按当前 `<html lang>` 本地化。

use dioxus::prelude::*;

use crate::i18n::format_localized_time;

/// 以 `<time datetime>` 展示 RFC3339 时间。
/// `value` 须可被 `format_localized_time` 解析；无法解析时显示「—」。`class` 供调用方叠加
/// 状态样式（如倒计时紧迫态）。
#[component]
pub fn Time(value: String, #[props(default)] class: Option<String>) -> Element {
    let formatted = format_localized_time(&value);

    rsx! {
        time { datetime: value, class, "{formatted}" }
    }
}
