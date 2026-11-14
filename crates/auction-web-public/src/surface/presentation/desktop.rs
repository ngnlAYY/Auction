//! 桌面导航、装备类型标签和账户章节入口；只消费页面投影与事件。

use crate::surface::{CATALOG_CATEGORIES, catalog_category_label};
use auction_web_shared::t;
use dioxus::prelude::*;

#[component]
pub(crate) fn CategoryFilter(
    active: String,
    counts: [i64; 8],
    on_change: EventHandler<String>,
) -> Element {
    rsx! {
        nav { class: "public-auction-filter public-desktop", aria_label: t!("filter-label"),
            span { class: "public-auction-filter__label", {t!("filter-equipment-type")} }
            button {
                class: "public-auction-filter__tab",
                r#type: "button",
                aria_current: if active == "all" { "true" } else { "false" },
                onclick: move |_| on_change.call("all".to_owned()),
                {t!("filter-all")}
                span { class: "public-auction-filter__count", "{counts[0]}" }
            }
            for (index, (category, _)) in CATALOG_CATEGORIES.into_iter().enumerate() {
                button {
                    class: "public-auction-filter__tab",
                    r#type: "button",
                    aria_current: if active == category { "true" } else { "false" },
                    onclick: move |_| on_change.call(category.to_owned()),
                    {catalog_category_label(category)}
                    span { class: "public-auction-filter__count", "{counts[index + 1]}" }
                }
            }
        }
    }
}
