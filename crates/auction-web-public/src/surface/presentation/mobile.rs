//! 手机导航、原生装备分组/类型选择和账户章节折叠入口；不持有查询、草稿或命令。

use auction_web_shared::t;
use dioxus::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

use super::{AccountSectionLinks, NavigationLinks};
use crate::surface::{CATALOG_CATEGORIES, catalog_category_label, equipment_group_label};

#[component]
pub(crate) fn Navigation(
    auction_home_href: String,
    reset_home_query: bool,
    open: bool,
    on_toggle: EventHandler<()>,
    on_close: EventHandler<()>,
    on_home: EventHandler<()>,
) -> Element {
    rsx! {
        button {
            id: "public-navigation-toggle",
            class: "public-shell__nav-toggle public-mobile",
            r#type: "button",
            aria_controls: "public-primary-navigation",
            aria_expanded: open.to_string(),
            onclick: move |_| on_toggle.call(()),
            onkeydown: move |event| {
                if open && event.key() == Key::Escape {
                    event.prevent_default();
                    event.stop_propagation();
                    on_close.call(());
                }
            },
            {t!("nav-label")}
        }
        nav {
            id: "public-primary-navigation",
            class: if open {
                "public-shell__nav public-shell__nav--open public-mobile"
            } else {
                "public-shell__nav public-mobile"
            },
            aria_label: t!("nav-label"),
            onclick: move |_| on_close.call(()),
            onkeydown: move |event| {
                if event.key() == Key::Escape {
                    event.prevent_default();
                    event.stop_propagation();
                    on_close.call(());
                    #[cfg(target_arch = "wasm32")]
                    if let Some(button) = web_sys::window()
                        .and_then(|window| window.document())
                        .and_then(|document| document.get_element_by_id("public-navigation-toggle"))
                        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
                    {
                        let _ = button.focus();
                    }
                }
            },
            NavigationLinks { auction_home_href, reset_home_query, on_home }
        }
    }
}

/// 八个类型选项用原生选择器替代多行标签；计数仍来自同一已校验目录。
#[component]
pub(crate) fn CategoryFilter(
    active: String,
    counts: [i64; 8],
    on_change: EventHandler<String>,
) -> Element {
    rsx! {
        label { class: "public-catalog-category-picker public-mobile", r#for: "public-catalog-category",
            span { {t!("filter-equipment-type")} }
            select {
                id: "public-catalog-category",
                name: "category",
                value: active,
                onchange: move |event| on_change.call(event.value()),
                option { value: "all", {t!("filter-all")}, " ({counts[0]})" }
                for (index, (category, _)) in CATALOG_CATEGORIES.into_iter().enumerate() {
                    option { value: category, {catalog_category_label(category)}, " ({counts[index + 1]})" }
                }
            }
        }
    }
}

/// 手机装备分组与类型同为原生选择器；与桌面分组标签消费同一当前值和回调，不持有筛选状态。
#[component]
pub(crate) fn EquipmentGroupFilter(active: String, on_change: EventHandler<String>) -> Element {
    rsx! {
        label { class: "public-catalog-category-picker public-mobile", r#for: "public-catalog-equipment-group",
            span { {t!("equipment-group")} }
            select {
                id: "public-catalog-equipment-group",
                name: "equipment_group",
                value: active,
                onchange: move |event| on_change.call(event.value()),
                option { value: "all", {t!("filter-all")} }
                for equipment_group in ["rare", "fusion"] {
                    option { value: equipment_group, {equipment_group_label(equipment_group)} }
                }
            }
        }
    }
}

#[component]
pub(crate) fn AccountNavigation() -> Element {
    rsx! {
        details { class: "public-account-navigation public-mobile",
            summary { {t!("account-section-navigation")} }
            nav { aria_label: t!("account-section-navigation"),
                AccountSectionLinks {}
            }
        }
    }
}

/// 长筛选面板底部仍提交原表单，不创建第二套草稿或查询。
#[component]
pub(crate) fn HistoryApply() -> Element {
    rsx! {
        button {
            r#type: "submit",
            class: "public-history-filter-actions__apply public-mobile",
            {t!("history-filter-apply")}
        }
    }
}
