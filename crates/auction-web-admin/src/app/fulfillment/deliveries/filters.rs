//! Delivery 状态筛选器组件。

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::helpers::{DELIVERY_STATUS_VALUES, delivery_status_label};

/// 选择状态后立即回到第一页；查询资源由父页面响应状态变化。
#[component]
pub(super) fn DeliveryFilters(
    mut status_filter: Signal<String>,
    mut page_number: Signal<i64>,
) -> Element {
    rsx! {
        section { class: "admin-delivery__filters", aria_labelledby: "delivery-filter-label",
            label { id: "delivery-filter-label",
                {t!("delivery-filter")}
                select {
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
