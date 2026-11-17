//! Admin realtime 运维页的总览指标卡：计数取值，以及按事件、消费者与失败分区语义的状态分级。

use auction_web_shared::t;
use dioxus::prelude::*;

pub(super) fn metric_card(label: String, value: String, state: &'static str) -> Element {
    rsx! {
        div { class: "admin-realtime-metric", "data-state": state,
            strong { class: "admin-realtime-metric__value", "{value}" }
            span { class: "admin-realtime-metric__label", "{label}" }
        }
    }
}

pub(super) fn metric_value(value: Option<i64>) -> String {
    value.map_or_else(|| t!("value-empty"), |value| value.to_string())
}

pub(super) fn activity_count_state(value: Option<i64>) -> &'static str {
    match value {
        None => "unavailable",
        Some(0) => "neutral",
        Some(_) => "ok",
    }
}

pub(super) fn consumer_count_state(value: Option<i64>) -> &'static str {
    match value {
        None => "unavailable",
        Some(0) => "warn",
        Some(_) => "ok",
    }
}

pub(super) fn failure_count_state(value: Option<i64>) -> &'static str {
    match value {
        None => "unavailable",
        Some(0) => "ok",
        Some(_) => "error",
    }
}
