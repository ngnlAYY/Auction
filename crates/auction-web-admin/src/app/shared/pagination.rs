//! Admin 列表共用分页组件。
//!
//! 旧版所有长列表都支持上一页、下一页和直接跳页。组件只处理有限的页码输入，实际数据
//! 仍由各页面自己的资源请求读取，避免浏览器持有或推导列表快照。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

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
    let mut jump_page = use_signal(|| page.to_string());

    use_effect(move || jump_page.set(page.to_string()));
    use_effect(move || {
        let valid_page = page.clamp(1, total_pages);
        if page != valid_page {
            on_page_change.call(valid_page);
        }
    });

    let clean = jump_page().trim().to_owned();
    let target = (!clean.is_empty() && clean.bytes().all(|value| value.is_ascii_digit()))
        .then(|| clean.parse::<i64>().ok())
        .flatten()
        .filter(|value| (1..=total_pages).contains(value));
    let can_jump = target.is_some();

    rsx! {
        nav { class: "ui-pager", aria_label: label,
            div { class: "ui-pager__controls",
                button {
                    r#type: "button",
                    disabled: page <= 1,
                    onclick: move |_| on_page_change.call(page - 1),
                    {t!("admin-pagination-previous")}
                }
                span { class: "ui-pager__status", aria_live: "polite",
                    {t!("admin-pagination-status", page: page, pages: total_pages)}
                }
                button {
                    r#type: "button",
                    disabled: page >= total_pages,
                    onclick: move |_| on_page_change.call(page + 1),
                    {t!("admin-pagination-next")}
                }
            }
            form {
                class: "ui-pager__jump",
                novalidate: true,
                onsubmit: move |event| {
                    event.prevent_default();
                    if let Some(target) = target {
                        on_page_change.call(target);
                    }
                },
                label { class: "ui-pager__label",
                    span { {t!("admin-pagination-input")} }
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
                button {
                    r#type: "submit",
                    disabled: !can_jump,
                    {t!("admin-pagination-go")}
                }
            }
        }
    }
}
