//! Public 长列表共用分页组件。
//!
//! 旧版共享 `Pager` 同时提供上一页、下一页和直接跳页。组件支持后端返回总数的稳定分页，
//! 也能在只有 `has_more` 的游标兼容投影上把跳转范围限制到当前已知的下一页。

use dioxus::prelude::*;
use dioxus_i18n::t;

/// 展示公开端长列表分页，并把目标页交给调用页面更新 URL 或查询状态。
#[component]
pub(super) fn PublicPager(
    page: i64,
    page_size: i64,
    total: Option<i64>,
    has_more: bool,
    label: String,
    on_page_change: EventHandler<i64>,
) -> Element {
    let page_size = page_size.max(1);
    let requested_page = page.max(1);
    let total_pages = total
        .map(|total| if total <= 0 { 1 } else { total.saturating_add(page_size - 1) / page_size })
        .unwrap_or_else(|| if has_more { requested_page.saturating_add(1) } else { requested_page })
        .max(1);
    let current_page = requested_page.min(total_pages);
    let mut jump_page = use_signal(|| current_page.to_string());

    use_effect(move || {
        jump_page.set(current_page.to_string());
        if requested_page != current_page {
            on_page_change.call(current_page);
        }
    });

    if total_pages <= 1 && current_page <= 1 {
        return rsx! {};
    }

    rsx! {
        nav { class: "public-auction-pages", aria_label: label,
            div { class: "public-auction-pages__controls",
                button {
                    r#type: "button",
                    disabled: current_page <= 1,
                    onclick: move |_| on_page_change.call(current_page - 1),
                    {t!("catalog-previous")}
                }
                span { class: "public-auction-pages__status", aria_live: "polite",
                    {t!("catalog-page", page: current_page, pages: total_pages)}
                }
                button {
                    r#type: "button",
                    disabled: current_page >= total_pages,
                    onclick: move |_| on_page_change.call(current_page + 1),
                    {t!("catalog-next")}
                }
            }
            form {
                class: "public-auction-pages__jump",
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
                    span { class: "public-visually-hidden", {t!("catalog-page-input")} }
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
                button { r#type: "submit", {t!("catalog-page-go")} }
            }
        }
    }
}
