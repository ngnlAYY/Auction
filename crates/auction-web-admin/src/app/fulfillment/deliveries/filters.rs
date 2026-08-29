//! Delivery 状态筛选器组件。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use super::helpers::{DELIVERY_STATUS_VALUES, delivery_status_label};

/// 选择状态后立即回到第一页；查询资源由父页面响应状态变化。
#[component]
pub(super) fn DeliveryFilters(
    mut status_filter: Signal<String>,
    mut page_number: Signal<i64>,
) -> Element {
    rsx! {
        div { class: "admin-delivery-filter",
            label { r#for: "delivery-status",
                {t!("delivery-filter")}
                select {
                    id: "delivery-status",
                    value: status_filter(),
                    onchange: move |event| {
                        status_filter.set(event.value());
                        page_number.set(1);
                    },
                    option { value: "", {t!("delivery-all")} }
                    for status in DELIVERY_STATUS_VALUES {
                        option {
                            value: status,
                            {delivery_status_label(status)}
                        }
                    }
                }
            }
        }
    }
}
