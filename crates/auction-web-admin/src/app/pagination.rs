//! Admin 列表共用分页组件。
//!
//! 旧版所有长列表都支持上一页、下一页和直接跳页。组件只处理有限的页码输入，实际数据
//! 仍由各页面自己的资源请求读取，避免浏览器持有或推导列表快照。

use dioxus::prelude::*;
use dioxus_i18n::t;

/// 展示带直接跳页输入的 Admin 分页控件。
#[component]
pub(super) fn AdminPager(
    page: i64,
    page_size: i64,
    total: i64,
    label: String,
    on_page_change: EventHandler<i64>,
) -> Element {
    let page_size = page_size.max(1);
    let total_pages = if total <= 0 { 1 } else { total.saturating_add(page_size - 1) / page_size };
    let current_page = page.clamp(1, total_pages);
    let mut jump_page = use_signal(|| current_page.to_string());

    use_effect(move || jump_page.set(current_page.to_string()));

    rsx! {
        nav { class: "admin-pagination", aria_label: label,
            div { class: "admin-pagination__controls",
                button {
                    r#type: "button",
                    disabled: current_page <= 1,
                    onclick: move |_| on_page_change.call(current_page - 1),
                    {t!("admin-pagination-previous")}
                }
                span { class: "admin-pagination__status", aria_live: "polite",
                    {t!("admin-pagination-status", page: current_page, pages: total_pages)}
                }
                button {
                    r#type: "button",
                    disabled: current_page >= total_pages,
                    onclick: move |_| on_page_change.call(current_page + 1),
                    {t!("admin-pagination-next")}
                }
            }
            form {
                class: "admin-pagination__jump",
                onsubmit: move |event| {
                    event.prevent_default();
                    let Ok(target) = jump_page().trim().parse::<i64>() else {
                        return;
                    };
                    if (1..=total_pages).contains(&target) {
                        on_page_change.call(target);
                    }
                },
                label {
                    span { class: "admin-visually-hidden", {t!("admin-pagination-input")} }
                    input {
                        r#type: "number",
                        min: "1",
                        max: total_pages.to_string(),
                        step: "1",
                        inputmode: "numeric",
                        value: jump_page(),
                        oninput: move |event| jump_page.set(event.value()),
                    }
                }
                button { r#type: "submit", {t!("admin-pagination-go")} }
            }
        }
    }
}
