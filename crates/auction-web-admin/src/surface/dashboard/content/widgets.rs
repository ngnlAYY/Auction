//! Dashboard 面板共用的卡片、徽标与纯文本渲染件。
//! 只统一 class 与语义属性，不携带任何指标判断，保持旧版 DOM 形状。

use auction_web_shared::t;
use dioxus::prelude::*;

pub(super) fn card(label: String, value: Element) -> Element {
    rsx! {
        div { class: "admin-dashboard-card",
            div { class: "admin-dashboard-value", {value} }
            div { class: "admin-dashboard-label", "{label}" }
        }
    }
}

pub(super) fn semantic_card(
    id: &'static str,
    label: String,
    state: String,
    value: Element,
) -> Element {
    rsx! {
        div { class: "admin-dashboard-card", id, "data-state": state,
            div { class: "admin-dashboard-value", {value} }
            div { class: "admin-dashboard-label", "{label}" }
        }
    }
}

pub(super) fn badge(state: &'static str, value: String) -> Element {
    rsx! {
        span { class: "admin-dashboard-badge admin-dashboard-badge--{state}", "{value}" }
    }
}

pub(super) fn text(value: String) -> Element {
    rsx! { "{value}" }
}

pub(super) fn count_text(value: Option<i64>) -> String {
    value.map_or_else(|| t!("dashboard-legacy-no-data"), |count| count.to_string())
}
