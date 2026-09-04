//! Public 长列表共用分页组件。
//!
//! 旧版共享 `Pager` 同时提供上一页、下一页和直接跳页。组件支持后端返回总数的稳定分页，
//! 也能在只有 `has_more` 的游标兼容投影上把跳转范围限制到当前已知的下一页。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

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
    let total_pages = total
        .map(|total| if total <= 0 { 1 } else { total.saturating_add(page_size - 1) / page_size })
        .unwrap_or_else(|| if has_more { page.saturating_add(1) } else { page })
        .max(1);
    // 先把页码钳制到当前已知合法范围，再渲染与派生交互；越界场景（如分享链接带上
    // 过大的 page）下，上一页/下一页/跳页等变更全部从钳制后的页码出发，不会先按
    // 原始越界页触发一次请求、再由下方纠偏 effect 重发第二次请求。
    let valid_page = page.clamp(1, total_pages);
    let mut jump_page = use_signal(|| valid_page.to_string());

    use_effect(move || jump_page.set(valid_page.to_string()));
    // 页码越界时只补一次父级状态/URL 纠偏；effect 仅在挂载后运行一次，重复收敛天然去重。
    use_effect(move || {
        if page != valid_page {
            on_page_change.call(valid_page);
        }
    });

    let clean = jump_page().trim().to_owned();
    let jump_target = (!clean.is_empty() && clean.bytes().all(|value| value.is_ascii_digit()))
        .then(|| clean.parse::<i64>().ok())
        .flatten()
        .filter(|target| (1..=total_pages).contains(target));
    let can_jump = jump_target.is_some();

    rsx! {
        nav { class: "ui-pager", aria_label: label,
            div { class: "ui-pager__controls",
                button {
                    r#type: "button",
                    disabled: valid_page <= 1,
                    onclick: move |_| on_page_change.call(valid_page - 1),
                    {t!("catalog-previous")}
                }
                span { class: "ui-pager__status", aria_live: "polite",
                    {t!("catalog-page", page: valid_page, pages: total_pages)}
                }
                button {
                    r#type: "button",
                    disabled: valid_page >= total_pages,
                    onclick: move |_| on_page_change.call(valid_page + 1),
                    {t!("catalog-next")}
                }
            }
            form {
                class: "ui-pager__jump",
                novalidate: true,
                onsubmit: move |event| {
                    event.prevent_default();
                    if let Some(target) = jump_target {
                        on_page_change.call(target);
                    }
                },
                label { class: "ui-pager__label",
                    span { {t!("catalog-page-input")} }
                    input {
                        class: "ui-pager__input",
                        r#type: "number",
                        min: "1",
                        max: total_pages.to_string(),
                        step: "1",
                        inputmode: "numeric",
                        value: jump_page(),
                        oninput: move |event| jump_page.set(event.value()),
                    }
                }
                button { r#type: "submit", disabled: !can_jump, {t!("catalog-page-go")} }
            }
        }
    }
}
