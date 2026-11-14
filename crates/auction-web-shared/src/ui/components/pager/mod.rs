//! Public 与 Admin 长列表共用分页器。
//! 支持后端返回总数的稳定分页，也支持只有 `has_more` 的游标兼容投影（跳转范围限制到当前
//! 已知的下一页）。文案一律由调用方经 props 传入，本 crate 不持有任何 i18n key；DOM class
//! 保持 `ui-pager*` 契约不变。调用方显式提供每页数量选项时才启用数字页码布局。
//! 页码导航位于数量和跳页之前；数字组独立换行，响应式样式不复制或重挂载表单。

use dioxus::prelude::*;

mod size;

use size::PageSizePicker;

/// 展示长列表分页，并把目标页交给调用页面更新 URL 或查询状态。
/// - `total: Some(total)`：后端返回总数的稳定分页；
/// - `total: None` + `has_more`：游标语义的兼容投影，可跳转范围限制到当前已知下一页。
/// 钳制契约（不得改）：渲染期先把 `page` 钳制到合法范围得到 `valid_page`，所有交互从
/// 钳制后页码出发；页码或总量变化后由纠偏 effect 通知父级把状态与 URL 收敛到合法页。
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
    /// 可访问页码上限；不改变权威 total，只限制后端 offset 边界内的导航。
    #[props(default)]
    max_page: Option<i64>,
    /// `nav` 的 `aria-label`。
    label: String,
    previous_label: String,
    /// 页码状态模板回调：`(当前页, 总页数) -> 已翻译文案`。
    status: Callback<(i64, i64), String>,
    next_label: String,
    jump_label: String,
    jump_button_label: String,
    on_page_change: EventHandler<i64>,
    /// 非用户操作的页码纠偏；URL 调用方可替换历史记录，省略时沿用 `on_page_change`。
    #[props(default)]
    on_page_clamp: Option<EventHandler<i64>>,
    /// 非空时启用数字分页与原生数量选择器；选项和文案归调用方。
    #[props(default)]
    page_size_options: Vec<i64>,
    #[props(default)] page_size_label: String,
    #[props(default)] page_size_option_label: Option<Callback<i64, String>>,
    /// 可选业务数量上限；默认无限制，保持 Public 既有语义。
    #[props(default)]
    max_page_size: Option<i64>,
    /// 显式启用正整数自定义输入，不改变默认布局。
    #[props(default)]
    allow_custom_page_size: bool,
    #[props(default)] page_size_custom_label: String,
    #[props(default)] page_size_apply_label: String,
    #[props(default)] page_size_invalid_label: String,
    /// 仅有权威 total 时由调用方提供总数文案，不把当前页长度当总数。
    #[props(default)]
    total_summary: String,
    /// 数字页码布局左端的条目范围文案；参数为 (起始序号, 结束序号, 总数)。
    /// 提供后替代 `total_summary`，避免同一工具条重复显示总数。
    #[props(default)]
    range_summary: Option<Callback<(i64, i64, i64), String>>,
    #[props(default)] on_page_size_change: EventHandler<i64>,
) -> Element {
    let page_size = page_size.max(1);
    let total_pages = total
        .map_or_else(
            || if has_more { page.saturating_add(1) } else { page },
            |total| if total <= 0 { 1 } else { 1 + (total - 1) / page_size },
        )
        .max(1)
        .min(max_page.unwrap_or(i64::MAX).max(1));
    // 先把页码钳制到合法范围再渲染与派生交互；输入同步与纠偏显式订阅 props。
    let valid_page = page.clamp(1, total_pages);
    let mut jump_page = use_signal(|| valid_page.to_string());

    use_effect(use_reactive((&valid_page,), move |(valid_page,)| {
        jump_page.set(valid_page.to_string());
    }));
    // 普通 props 不会被 effect 自动追踪；总量收缩后也须让父级重新读取有效页。
    use_effect(use_reactive((&page, &valid_page), move |(page, valid_page)| {
        if page != valid_page {
            on_page_clamp.unwrap_or(on_page_change).call(valid_page);
        }
    }));

    let jump_target = {
        let value = jump_page.read();
        let clean = value.trim();
        (!clean.is_empty() && clean.bytes().all(|value| value.is_ascii_digit()))
            .then(|| clean.parse::<i64>().ok())
            .flatten()
            .filter(|target| (1..=total_pages).contains(target))
    };
    let can_jump = jump_target.is_some();
    let numbered = !page_size_options.is_empty();
    // 范围按钳制后的有效页计算，总数收缩的瞬间不会显示越界序号；空列表固定为 0–0。
    let item_range = total.map(|total_items| {
        if total_items <= 0 {
            (0, 0, 0)
        } else {
            let start = (valid_page - 1).saturating_mul(page_size).saturating_add(1);
            let end = valid_page.saturating_mul(page_size).min(total_items);
            (start, end, total_items)
        }
    });
    let range_rendered = numbered && item_range.is_some() && range_summary.is_some();

    rsx! {
        nav {
            class: if numbered { "ui-pager ui-pager--numbered" } else { "ui-pager" },
            aria_label: label,
            if numbered {
                if let (Some((start, end, count)), Some(template)) = (item_range, range_summary) {
                    span { class: "ui-pager__range", {template.call((start, end, count))} }
                }
            }
            div { class: "ui-pager__controls",
                button {
                    r#type: "button",
                    disabled: valid_page <= 1,
                    onclick: move |_| on_page_change.call(valid_page - 1),
                    aria_label: previous_label,
                    if numbered {
                        span { aria_hidden: "true", "‹" }
                    } else {
                        "{previous_label}"
                    }
                }
                if numbered {
                    div { class: "ui-pager__pages",
                        for target in numbered_pages(valid_page, total_pages) {
                            if let Some(target) = target {
                                button {
                                    key: "page-{target}",
                                    r#type: "button",
                                    class: "ui-pager__page",
                                    aria_label: status.call((target, total_pages)),
                                    aria_current: if target == valid_page { "page" } else { "false" },
                                    onclick: move |_| {
                                        if target != valid_page {
                                            on_page_change.call(target);
                                        }
                                    },
                                    "{target}"
                                }
                            } else {
                                span { class: "ui-pager__ellipsis", aria_hidden: "true", "…" }
                            }
                        }
                    }
                }
                span {
                    class: if numbered { "ui-pager__sr-only" } else { "ui-pager__status" },
                    aria_live: "polite",
                    {status.call((valid_page, total_pages))}
                }
                button {
                    r#type: "button",
                    disabled: valid_page >= total_pages,
                    onclick: move |_| on_page_change.call(valid_page.saturating_add(1)),
                    aria_label: next_label,
                    if numbered {
                        span { aria_hidden: "true", "›" }
                    } else {
                        "{next_label}"
                    }
                }
            }
            if numbered {
                if !range_rendered && total.is_some() && !total_summary.is_empty() {
                    span { class: "ui-pager__total", "{total_summary}" }
                }
                PageSizePicker {
                    page_size,
                    options: page_size_options,
                    label: page_size_label,
                    option_label: page_size_option_label,
                    allow_custom: allow_custom_page_size,
                    max_page_size,
                    custom_label: page_size_custom_label,
                    apply_label: page_size_apply_label,
                    invalid_label: page_size_invalid_label,
                    on_change: on_page_size_change,
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
                        name: "page",
                        min: "1",
                        max: total_pages.to_string(),
                        step: "1",
                        inputmode: "numeric",
                        autocomplete: "off",
                        enterkeyhint: "go",
                        value: jump_page(),
                        oninput: move |event| jump_page.set(event.value()),
                    }
                }
                button { r#type: "submit", disabled: !can_jump, "{jump_button_label}" }
            }
        }
    }
}

/// 最多生成七个页码/省略项；固定栈缓冲不分配临时 Vec，省略号仍不可点击。
fn numbered_pages(page: i64, total_pages: i64) -> impl Iterator<Item = Option<i64>> {
    let pages = if total_pages <= 7 {
        [Some(1), Some(2), Some(3), Some(4), Some(5), Some(6), Some(7)]
    } else if page <= 4 {
        [Some(1), Some(2), Some(3), Some(4), Some(5), None, Some(total_pages)]
    } else if page >= total_pages - 3 {
        [
            Some(1),
            None,
            Some(total_pages - 4),
            Some(total_pages - 3),
            Some(total_pages - 2),
            Some(total_pages - 1),
            Some(total_pages),
        ]
    } else {
        [Some(1), None, Some(page - 1), Some(page), Some(page + 1), None, Some(total_pages)]
    };
    pages.into_iter().take(total_pages.clamp(0, 7) as usize)
}
