//! Delivery 状态筛选器组件。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use super::helpers::{DELIVERY_STATUS_VALUES, delivery_status_label};

/// 选择状态后由父页面写回地址栏并回到第一页；查询资源响应查询状态变化自动重新加载。
#[component]
pub(super) fn DeliveryFilters(
    /// 当前状态筛选（地址栏恢复后的值）。
    status: String,
    on_status_change: EventHandler<String>,
) -> Element {
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
        }
    }
}
