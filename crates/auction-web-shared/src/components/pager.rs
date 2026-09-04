//! Public 与 Admin 长列表共用分页器。
//!
//! 合并自两端原先近逐字重复的 `PublicPager` / `AdminPager`。组件同时提供上一页、下一页和
//! 直接跳页：支持后端返回总数的稳定分页，也能在只有 `has_more` 的游标兼容投影上把跳转
//! 范围限制到当前已知的下一页。
//!
//! 文案一律由调用方通过 props 传入（含「第 X 页 / 共 Y 页」状态模板回调），本 crate
//! 不持有任何 i18n key；DOM class 保持 `ui-pager*` 契约不变。

use dioxus::prelude::*;

/// 展示长列表分页，并把目标页交给调用页面更新 URL 或查询状态。
///
/// 分页语义：
/// - `total: Some(total)`：后端返回总数的稳定分页；
/// - `total: None` + `has_more`：只有游标语义的兼容投影，可跳转范围限制到当前已知的下一页。
///
/// 钳制契约（两端迁移时必须保留的行为）：渲染期先把 `page` 钳制到当前已知合法范围
/// 得到 `valid_page`，上一页/下一页/跳页等交互全部从钳制后的页码出发；越界场景
/// （如分享链接带上过大的 page）不会先按原始越界页触发一次请求，挂载后的纠偏
/// effect 只补一次父级状态/URL 收敛。
#[component]
pub fn Pager(
    page: i64,
    page_size: i64,
    /// 后端返回的总条数；`None` 表示只有 `has_more` 游标语义。
    #[props(default)]
    total: Option<i64>,
    /// 游标语义下是否还有下一页；仅在 `total` 为 `None` 时参与计算。
    #[props(default)]
    has_more: bool,
    /// `nav` 的 `aria-label`。
    label: String,
    /// 上一页按钮文案。
    previous_label: String,
    /// 页码状态模板回调：`(当前页, 总页数) -> 已翻译文案`。
    status: Callback<(i64, i64), String>,
    /// 下一页按钮文案。
    next_label: String,
    /// 跳页输入框的标签文案。
    jump_label: String,
    /// 跳页提交按钮文案。
    jump_button_label: String,
    on_page_change: EventHandler<i64>,
) -> Element {
    let page_size = page_size.max(1);
    let total_pages = total
        .map(|total| if total <= 0 { 1 } else { total.saturating_add(page_size - 1) / page_size })
        .unwrap_or_else(|| if has_more { page.saturating_add(1) } else { page })
        .max(1);
    // 先把页码钳制到当前已知合法范围，再渲染与派生交互；Dioxus 0.7 不追踪普通 props，
    // 越界场景下上一页/下一页/跳页全部从钳制后的页码出发，不会先按原始越界页触发一次请求。
    let valid_page = page.clamp(1, total_pages);
    let mut jump_page = use_signal(|| valid_page.to_string());

    use_effect(move || jump_page.set(valid_page.to_string()));
    // 页码越界时只补一次父级状态纠偏；effect 仅在挂载后运行一次，重复收敛天然去重。
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
                    "{previous_label}"
                }
                span { class: "ui-pager__status", aria_live: "polite",
                    {status.call((valid_page, total_pages))}
                }
                button {
                    r#type: "button",
                    disabled: valid_page >= total_pages,
                    onclick: move |_| on_page_change.call(valid_page + 1),
                    "{next_label}"
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
                    span { "{jump_label}" }
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
                button { r#type: "submit", disabled: !can_jump, "{jump_button_label}" }
            }
        }
    }
}
