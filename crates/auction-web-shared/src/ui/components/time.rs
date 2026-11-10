//! 本地化时间展示组件。
//! 机器可读的 `datetime` 保留原始 RFC3339 值，可见文本直接订阅当前 I18n 语言。

use dioxus::prelude::*;
use dioxus_i18n::prelude::i18n;

use crate::i18n::format_localized_time_for_language;

/// 以 `<time datetime>` 展示 RFC3339 时间。
/// `value` 须可被时间格式化函数解析；无法解析时显示「—」。`class` 供调用方叠加
/// 状态样式（如倒计时紧迫态）。
#[component]
pub fn Time(value: String, #[props(default)] class: Option<String>) -> Element {
    let language = i18n().language();
    let formatted = format_localized_time_for_language(&value, &language.to_string());

    rsx! {
        time { datetime: value, class, "{formatted}" }
    }
}
