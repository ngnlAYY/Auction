//! Public 与 Admin 长列表共用分页器。
//! 文案一律由调用方经 props 传入，本 crate 不持有任何 i18n key；DOM class
//! 保持 `ui-pager*` 契约不变。调用方显式提供每页数量选项时才启用数字页码布局。
//! 页码导航位于数量和跳页之前；数字组独立换行，响应式样式不复制或重挂载表单。
//! 回调属性须为非可选类型，避免更新时拷入会随临时 VNode 释放的句柄
//! （Dioxus 0.7.10 的 `dioxus-core-macro/src/props/mod.rs:694–715` 与 `dioxus-core/src/events.rs:555–556`）。

use dioxus::dioxus_core::{SpawnIfAsync, SuperFrom};
use dioxus::prelude::*;

use crate::validation::restore_input_value;

mod size;

use size::PageSizePicker;

/// 标记分页调用方给出的单位返回值，保留现有闭包与 EventHandler 传参。
#[doc(hidden)]
pub struct PaginationCallbackHandled;

impl SpawnIfAsync<PaginationCallbackHandled, bool> for () {
    fn spawn(self) -> bool {
        true
    }
}

impl<Args: 'static> SuperFrom<EventHandler<Args>, PaginationCallbackHandled>
    for Callback<Args, bool>
{
    fn super_from(handler: EventHandler<Args>) -> Self {
        Callback::new(move |args| {
            handler.call(args);
            true
        })
    }
}

/// 展示长列表分页，并把目标页交给调用页面更新 URL 或查询状态。
/// - `total: Some(total)`：后端返回总数的稳定分页；
/// - `total: None` + `has_more`：游标语义的兼容投影，可跳转范围限制到当前已知下一页。
/// 导航范围按钳制后的页码计算；页码或总量变化后由 effect 通知父级纠偏。
/// 调用方提供失败文案时，显示页码以父级已提交结果为准，失败后允许重试。
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
    /// 由地址驱动的调用方可返回写入结果；缺省调用 `on_page_change`。
    #[props(default)]
    on_page_commit: Callback<i64, Option<bool>>,
    /// 非用户操作的页码纠偏。默认回调返回 false，沿用 `on_page_change`；返回 true 表示已纠偏。
    #[props(default)]
    on_page_clamp: Callback<i64, bool>,
    /// 错误与重试文案。提供时，纠偏被拒后保留已应用页码，并由用户重试。
    #[props(default)]
    clamp_failure: Option<(String, String)>,
    /// 非空时启用数字分页与原生数量选择器；选项和文案归调用方。
    #[props(default)]
    page_size_options: Vec<i64>,
    #[props(default)] page_size_label: String,
    #[props(default = |size: i64| size.to_string())] page_size_option_label: Callback<i64, String>,
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
    /// 数字页码布局有权威总数时用范围文案替代 `total_summary`，避免重复显示总数。
    #[props(default)]
    show_range_summary: bool,
    /// 条目范围文案，参数为 (起始序号, 结束序号, 总数)。
    #[props(default)]
    range_summary: Callback<(i64, i64, i64), String>,
    #[props(default)] on_page_size_change: EventHandler<i64>,
    /// 由地址驱动的调用方可返回写入结果；缺省调用 `on_page_size_change`。
    #[props(default)]
    on_page_size_commit: Callback<i64, Option<bool>>,
) -> Element {
    let page_size = page_size.max(1);
    let total_pages = total
        .map_or_else(
            || if has_more { page.saturating_add(1) } else { page },
            |total| if total <= 0 { 1 } else { 1 + (total - 1) / page_size },
        )
        .max(1)
        .min(max_page.unwrap_or(i64::MAX).max(1));
    let valid_page = page.clamp(1, total_pages);
    let applied_page = if clamp_failure.is_some() { page.max(1) } else { valid_page };
    let mut jump_page = use_signal(|| applied_page.to_string());
    let mut input_element = use_signal(|| None::<MountedEvent>);
    let mut clamp_rejected = use_signal(|| false);

    use_effect(use_reactive((&applied_page,), move |(applied_page,)| {
        jump_page.set(applied_page.to_string());
    }));
    let change_page = use_callback(move |target| {
        if let Some(written) = on_page_commit.call(target) {
            if !written {
                let value = applied_page.to_string();
                if let Some(element) = input_element.peek().as_ref() {
                    restore_input_value(element, &value);
                }
                jump_page.set(value);
            }
        } else {
            on_page_change.call(target);
        }
    });
    let retry_clamp = clamp_failure.is_some();
    let correct_page = use_callback(move |(page, valid_page)| {
        let rejected = page != valid_page && !on_page_clamp.call(valid_page);
        if retry_clamp {
            clamp_rejected.set(rejected);
        } else if rejected {
            on_page_change.call(valid_page);
        }
    });
    // 普通 props 不会被 effect 自动追踪；总量收缩后也须让父级重新读取有效页。
    use_effect(use_reactive((&page, &valid_page), move |(page, valid_page)| {
        correct_page.call((page, valid_page));
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
    // 总数收缩后仍待提交的页不显示条目序号；空列表固定为 0–0。
    let item_range = total.filter(|_| numbered && show_range_summary).and_then(|total_items| {
        if total_items <= 0 {
            Some((0, 0, 0))
        } else if applied_page == valid_page {
            let start = (valid_page - 1).saturating_mul(page_size).saturating_add(1);
            let end = valid_page.saturating_mul(page_size).min(total_items);
            Some((start, end, total_items))
        } else {
            None
        }
    });

    rsx! {
        nav {
            class: if numbered { "ui-pager ui-pager--numbered" } else { "ui-pager" },
            aria_label: label,
            if let Some(range) = item_range {
                span { class: "ui-pager__range", {range_summary.call(range)} }
            }
            div { class: "ui-pager__controls",
                button {
                    r#type: "button",
                    disabled: valid_page <= 1,
                    onclick: move |_| change_page.call(valid_page - 1),
                    aria_label: previous_label,
                    if numbered {
                        span { aria_hidden: "true", "‹" }
                    } else {
                        "{previous_label}"
                    }
                }
                if numbered {
                    div { class: "ui-pager__pages",
                        // 同一页码改变窗口位置时仍复用按钮，保留键盘焦点。
                        {numbered_pages(valid_page, total_pages).enumerate().map(|(index, target)| {
                            if let Some(target) = target {
                                rsx! {
                                    button {
                                        key: "page-{target}",
                                        r#type: "button",
                                        class: "ui-pager__page",
                                        aria_label: status.call((target, total_pages)),
                                        aria_current: if target == applied_page { "page" } else { "false" },
                                        onclick: move |_| {
                                            if target != applied_page {
                                                change_page.call(target);
                                            }
                                        },
                                        "{target}"
                                    }
                                }
                            } else {
                                rsx! {
                                    span { key: "ellipsis-{index}", class: "ui-pager__ellipsis", aria_hidden: "true", "…" }
                                }
                            }
                        })}
                    }
                }
                span {
                    class: if numbered { "ui-pager__sr-only" } else { "ui-pager__status" },
                    aria_live: "polite",
                    {status.call((applied_page, total_pages))}
                }
                button {
                    r#type: "button",
                    disabled: valid_page >= total_pages,
                    onclick: move |_| change_page.call(valid_page.saturating_add(1)),
                    aria_label: next_label,
                    if numbered {
                        span { aria_hidden: "true", "›" }
                    } else {
                        "{next_label}"
                    }
                }
            }
            if numbered {
                if item_range.is_none() && total.is_some() && !total_summary.is_empty() {
                    span { class: "ui-pager__total", "{total_summary}" }
                }
                PageSizePicker {
                    page_size,
                    options: page_size_options,
                    label: page_size_label,
                    // 选择器持有独立属性槽，格式闭包更新时会重新渲染选项文本。
                    option_label: move |size| page_size_option_label.call(size),
                    allow_custom: allow_custom_page_size,
                    max_page_size,
                    custom_label: page_size_custom_label,
                    apply_label: page_size_apply_label,
                    invalid_label: page_size_invalid_label,
                    on_change: move |size| on_page_size_commit.call(size).unwrap_or_else(|| {
                        on_page_size_change.call(size);
                        true
                    }),
                }
            }
            form {
                class: "ui-pager__jump",
                novalidate: true,
                onsubmit: move |event| {
                    event.prevent_default();
                    if let Some(target) = jump_target {
                        change_page.call(target);
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
                        onmounted: move |event| input_element.set(Some(event)),
                        value: jump_page(),
                        oninput: move |event| jump_page.set(event.value()),
                    }
                }
                button { r#type: "submit", disabled: !can_jump, "{jump_button_label}" }
            }
        }
        if let Some((message, retry_label)) = clamp_failure {
            div { class: "ui-pager__error", role: "status", aria_live: "polite",
                if page != valid_page && clamp_rejected() {
                    span { "{message}" }
                    button { r#type: "button", onclick: move |_| correct_page.call((page, valid_page)),
                        "{retry_label}"
                    }
                }
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

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    use dioxus::dioxus_core::{AttributeValue, Mutation, Mutations};
    use dioxus::prelude::*;

    use super::size::{PageSizePicker, PageSizePickerProps};
    use super::{Pager, PagerProps};

    enum PagerCallback {
        Clamp,
        ClampRejected,
        SizeLabel,
        Range,
        Window,
    }

    struct Fixture {
        callback: PagerCallback,
        revision: Cell<i64>,
        page_calls: RefCell<Vec<(i64, i64)>>,
    }

    fn pager_fixture() -> Element {
        let fixture = consume_context::<Rc<Fixture>>();
        let revision = fixture.revision.get();
        let fallback = fixture.clone();
        let (page, total) = if matches!(fixture.callback, PagerCallback::Window) {
            (4 + revision, 100)
        } else {
            (if matches!(revision, 1 | 2 | 5) { 4 } else { 10 + revision }, 40)
        };
        let props = PagerProps::builder()
            .page(page)
            .page_size(10)
            .total(Some(total))
            .label("翻页".to_owned())
            .previous_label("上一页".to_owned())
            .status(|(page, pages)| format!("{page}/{pages}"))
            .next_label("下一页".to_owned())
            .jump_label("页码".to_owned())
            .jump_button_label("跳转".to_owned())
            .on_page_change(move |page| fallback.page_calls.borrow_mut().push((-revision, page)))
            .page_size_options(vec![20])
            .total_summary("共 40 条".to_owned());
        let props = match (&fixture.callback, revision) {
            (PagerCallback::Clamp, 1..=3) => props
                .on_page_clamp(move |page| fixture.page_calls.borrow_mut().push((revision, page)))
                .build(),
            (PagerCallback::ClampRejected, _) => props
                .clamp_failure(Some(("页码未写入".to_owned(), "重试".to_owned())))
                .on_page_clamp(move |page| {
                    fixture.page_calls.borrow_mut().push((revision, page));
                    false
                })
                .build(),
            (PagerCallback::SizeLabel, 1..=3) => props
                .page_size_option_label(move |size| format!("{revision}：每页 {size} 条"))
                .build(),
            (PagerCallback::Range, 1..=3) => props
                .show_range_summary(true)
                .range_summary(move |(start, end, total)| {
                    format!("{revision}：{start}–{end} / {total}")
                })
                .build(),
            (PagerCallback::Range, 4) => props
                .range_summary(|_| -> String {
                    panic!("未启用范围时不能调用范围回调")
                })
                .build(),
            _ => props.build(),
        };
        rsx! { Pager { ..props } }
    }

    fn size_picker_fixture() -> Element {
        let revision = consume_context::<Rc<Cell<i64>>>().get();
        let props = PageSizePickerProps::builder()
            .page_size(10)
            .options(vec![20])
            .label("每页数量".to_owned())
            .allow_custom(false)
            .custom_label("自定义数量".to_owned())
            .apply_label("应用".to_owned())
            .invalid_label("数量无效".to_owned())
            .on_change(|_| true);
        let props = if (1..=3).contains(&revision) {
            props.option_label(move |size| format!("{revision}：每页 {size} 条")).build()
        } else {
            props.build()
        };
        rsx! { PageSizePicker { ..props } }
    }

    fn render_at(dom: &mut VirtualDom, revision: &Cell<i64>, value: i64) -> Mutations {
        revision.set(value);
        dom.mark_dirty(ScopeId::APP);
        dom.render_immediate_to_vec()
    }

    fn has_text(mutations: &Mutations, expected: &str) -> bool {
        mutations.edits.iter().any(|edit| {
            matches!(edit, Mutation::CreateTextNode { value, .. } | Mutation::SetText { value, .. }
                if value == expected)
        })
    }

    fn mount_pager(callback: PagerCallback) -> (VirtualDom, Rc<Fixture>, Mutations) {
        let fixture = Rc::new(Fixture {
            callback,
            revision: Cell::new(0),
            page_calls: RefCell::new(Vec::new()),
        });
        let mut dom = VirtualDom::new(pager_fixture).with_root_context(fixture.clone());
        let initial = dom.rebuild_to_vec();
        (dom, fixture, initial)
    }

    #[test]
    fn keyed_page_buttons_keep_their_node_when_the_window_shifts() {
        let (mut dom, fixture, initial) = mount_pager(PagerCallback::Window);
        let page_five = initial
            .edits
            .iter()
            .find_map(|edit| match edit {
                Mutation::SetAttribute {
                    id,
                    name: "aria-label",
                    value: AttributeValue::Text(label),
                    ..
                } if label == "5/10" => Some(*id),
                _ => None,
            })
            .expect("第 5 页按钮存在");
        let shifted = render_at(&mut dom, &fixture.revision, 1);
        assert!(shifted.edits.iter().any(|edit| matches!(
            edit,
            Mutation::SetAttribute { id, name: "aria-current", value: AttributeValue::Text(value), .. }
                if *id == page_five && value == "page"
        )), "窗口移动后原来的第 5 页按钮须保留，避免焦点丢失");
    }

    #[test]
    fn clamp_uses_the_latest_callback_and_defaults_to_page_change() {
        let (mut dom, fixture, _) = mount_pager(PagerCallback::Clamp);
        dom.render_immediate_to_vec();
        assert_eq!(&*fixture.page_calls.borrow(), &[(0, 4)]);
        for revision in 1..=5 {
            render_at(&mut dom, &fixture.revision, revision);
            dom.render_immediate_to_vec();
        }
        assert_eq!(&*fixture.page_calls.borrow(), &[(0, 4), (3, 4), (-4, 4)]);
    }

    #[test]
    fn failed_total_clamp_keeps_the_applied_page_without_falling_back_to_push() {
        let (mut dom, fixture, initial) = mount_pager(PagerCallback::ClampRejected);
        assert!(has_text(&initial, "10/4"));
        let edits = dom.render_immediate_to_vec();
        assert!(has_text(&edits, "页码未写入"));
        assert!(has_text(&edits, "重试"));
        assert_eq!(&*fixture.page_calls.borrow(), &[(0, 4)]);
    }

    #[test]
    fn pager_size_label_updates_and_defaults_to_decimal() {
        let (mut dom, fixture, initial) = mount_pager(PagerCallback::SizeLabel);
        assert!(has_text(&initial, "20"));
        assert!(has_text(&initial, "10"));
        for revision in 1..=2 {
            render_at(&mut dom, &fixture.revision, revision);
        }
        let latest = render_at(&mut dom, &fixture.revision, 3);
        assert!(has_text(&latest, "3：每页 20 条"));
        assert!(has_text(&latest, "3：每页 10 条"));
        let fallback = render_at(&mut dom, &fixture.revision, 4);
        assert!(has_text(&fallback, "20"));
        assert!(has_text(&fallback, "10"));
    }

    #[test]
    fn range_summary_updates_and_defaults_to_total_summary() {
        let (mut dom, fixture, initial) = mount_pager(PagerCallback::Range);
        assert!(has_text(&initial, "共 40 条"));
        for revision in 1..=3 {
            let current = render_at(&mut dom, &fixture.revision, revision);
            assert!(has_text(&current, &format!("{revision}：31–40 / 40")));
            assert!(!has_text(&current, "共 40 条"));
        }
        assert!(has_text(&render_at(&mut dom, &fixture.revision, 4), "共 40 条"));
    }

    #[test]
    fn picker_option_label_updates_and_defaults_to_decimal() {
        let revision = Rc::new(Cell::new(0));
        let mut dom = VirtualDom::new(size_picker_fixture).with_root_context(revision.clone());
        let initial = dom.rebuild_to_vec();
        assert!(has_text(&initial, "20"));
        assert!(has_text(&initial, "10"));
        for value in 1..=3 {
            let current = render_at(&mut dom, &revision, value);
            assert!(has_text(&current, &format!("{value}：每页 20 条")));
            assert!(has_text(&current, &format!("{value}：每页 10 条")));
        }
        let fallback = render_at(&mut dom, &revision, 4);
        assert!(has_text(&fallback, "20"));
        assert!(has_text(&fallback, "10"));
    }
}
