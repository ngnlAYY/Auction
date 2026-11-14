//! Delivery 检索筛选器组件：状态 + 收件人、参考编号、模板、类别、世界与时间。

use auction_web_shared::t;
use chrono::{NaiveDateTime, SecondsFormat, TimeZone, Utc};
use dioxus::prelude::*;

use super::labels::{DELIVERY_STATUS_VALUES, delivery_category_label, delivery_status_label};

/// 提交前的筛选草稿；只在「应用筛选」时写入地址栏，输入过程不触发网络请求。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct DeliveryFiltersDraft {
    pub recipient: String,
    pub reference: String,
    pub template: String,
    pub category: String,
    pub world: String,
    pub created_from: String,
    pub created_to: String,
}

const DELIVERY_CATEGORY_VALUES: [&str; 7] =
    ["outbid", "account", "intake", "settlement", "equipment", "operations", "manual"];

/// `datetime-local` 只接受分钟精度；把地址栏的 RFC3339 还原成该格式。
fn timestamp_to_local_input(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.format("%Y-%m-%dT%H:%M").to_string())
        .unwrap_or_default()
}

/// 把分钟精度的本地输入按 UTC 解释并序列化为服务端接受的 RFC3339。
fn local_input_to_timestamp(value: &str) -> String {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M")
        .ok()
        .map(|value| Utc.from_utc_datetime(&value).to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_default()
}

#[component]
pub(super) fn DeliveryFilters(
    /// 当前状态筛选（地址栏恢复后的值）。
    status: String,
    recipient: String,
    reference: String,
    template: String,
    category: String,
    world: String,
    created_from: String,
    created_to: String,
    on_status_change: EventHandler<String>,
    on_filters_change: EventHandler<DeliveryFiltersDraft>,
) -> Element {
    let mut draft = use_signal(|| DeliveryFiltersDraft {
        recipient: recipient.clone(),
        reference: reference.clone(),
        template: template.clone(),
        category: category.clone(),
        world: world.clone(),
        created_from: timestamp_to_local_input(&created_from),
        created_to: timestamp_to_local_input(&created_to),
    });
    rsx! {
        div { class: "admin-delivery-filter",
            label { r#for: "delivery-status",
                {t!("delivery-filter")}
                select {
                    id: "delivery-status",
                    value: status,
                    onchange: move |event| on_status_change.call(event.value()),
                    option { value: "", {t!("delivery-all")} }
                    for status in DELIVERY_STATUS_VALUES {
                        option {
                            value: status,
                            {delivery_status_label(status)}
                        }
                    }
                }
            }
            form {
                class: "admin-delivery-search",
                onsubmit: move |event| {
                    event.prevent_default();
                    let current = draft();
                    on_filters_change.call(DeliveryFiltersDraft {
                        recipient: current.recipient.trim().to_owned(),
                        reference: current.reference.trim().to_owned(),
                        template: current.template.trim().to_owned(),
                        category: current.category,
                        world: current.world,
                        created_from: local_input_to_timestamp(&current.created_from),
                        created_to: local_input_to_timestamp(&current.created_to),
                    });
                },
                label { r#for: "delivery-recipient",
                    span { {t!("delivery-search-recipient")} }
                    input {
                        id: "delivery-recipient",
                        r#type: "text",
                        maxlength: "256",
                        value: "{draft().recipient}",
                        oninput: move |event| draft.write().recipient = event.value(),
                    }
                }
                label { r#for: "delivery-reference",
                    span { {t!("delivery-search-reference")} }
                    input {
                        id: "delivery-reference",
                        r#type: "text",
                        maxlength: "36",
                        value: "{draft().reference}",
                        oninput: move |event| draft.write().reference = event.value(),
                    }
                }
                label { r#for: "delivery-template",
                    span { {t!("delivery-template")} }
                    input {
                        id: "delivery-template",
                        r#type: "text",
                        maxlength: "64",
                        value: "{draft().template}",
                        oninput: move |event| draft.write().template = event.value(),
                    }
                }
                label { r#for: "delivery-category",
                    span { {t!("delivery-category")} }
                    select {
                        id: "delivery-category",
                        value: "{draft().category}",
                        onchange: move |event| draft.write().category = event.value(),
                        option { value: "", {t!("delivery-all")} }
                        for value in DELIVERY_CATEGORY_VALUES {
                            option { value: value, {delivery_category_label(value)} }
                        }
                    }
                }
                label { r#for: "delivery-world",
                    span { {t!("delivery-world")} }
                    select {
                        id: "delivery-world",
                        value: "{draft().world}",
                        onchange: move |event| draft.write().world = event.value(),
                        option { value: "", {t!("delivery-all")} }
                        option { value: "main", "main" }
                        option { value: "isekai", "isekai" }
                    }
                }
                label { r#for: "delivery-created-from",
                    span { {t!("delivery-created-from")} }
                    input {
                        id: "delivery-created-from",
                        r#type: "datetime-local",
                        value: "{draft().created_from}",
                        oninput: move |event| draft.write().created_from = event.value(),
                    }
                }
                label { r#for: "delivery-created-to",
                    span { {t!("delivery-created-to")} }
                    input {
                        id: "delivery-created-to",
                        r#type: "datetime-local",
                        value: "{draft().created_to}",
                        oninput: move |event| draft.write().created_to = event.value(),
                    }
                }
                button { class: "admin-btn-primary", r#type: "submit", {t!("delivery-search-apply")} }
            }
        }
    }
}
