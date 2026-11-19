//! 目录列设置：类型工具栏右端的图标按钮与下拉面板（标题、全选、逐列勾选）。
//! 只改写本地列偏好，不改变目录查询、URL 或竞价状态；隐藏列不卸载表格命令宿主。

mod state;

pub(crate) use state::{CatalogColumn, CatalogLayout, CatalogVisibility};

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::surface::panel_escape::{focus_panel_trigger, use_panel_escape};

const TOGGLE_ID: &str = "public-catalog-columns-toggle";
const PANEL_ID: &str = "public-catalog-columns-panel";
const TITLE_ID: &str = "public-catalog-columns-title";
const ALL_ID: &str = "public-catalog-column-all";

/// 列设置入口与下拉面板。展开状态只属于本组件：勾选立即生效并写入本地偏好，
/// 没有待提交的输入，所以点面板外或按 Escape 直接关闭。
#[component]
pub(crate) fn CatalogColumnsMenu(
    mut visibility: Signal<CatalogVisibility>,
    layout: CatalogLayout,
) -> Element {
    let open = use_signal(|| false);
    let visible = visibility().for_layout(layout);
    let shown = visible.visible_count(layout);
    let all = usize::try_from(shown).is_ok_and(|shown| shown == layout.columns().len());
    let last_column = shown == 1;
    let mut toggle = open;
    let dismiss = use_callback(move |()| {
        if !*open.peek() {
            return false;
        }
        close(open);
        true
    });
    use_panel_escape(move || *open.read(), TOGGLE_ID, PANEL_ID, dismiss);

    rsx! {
        div { class: "public-catalog-columns-menu",
            button {
                id: TOGGLE_ID,
                class: "public-catalog-columns__toggle",
                r#type: "button",
                aria_controls: PANEL_ID,
                aria_expanded: if open() { "true" } else { "false" },
                aria_label: t!("catalog-columns-settings"),
                title: t!("catalog-columns-settings"),
                onclick: move |_| toggle.set(!toggle()),
                ColumnsIcon {}
            }
            if open() {
                // 透明遮罩接住面板外的点击。
                div { class: "public-catalog-columns__backdrop", onclick: move |_| { dismiss.call(()); } }
                div {
                    id: PANEL_ID,
                    class: "public-catalog-columns",
                    role: "group",
                    aria_labelledby: TITLE_ID,
                    "aria-description": t!("catalog-columns-hint"),
                    strong { id: TITLE_ID, class: "public-catalog-columns__title", {t!("catalog-columns")} }
                    label { r#for: ALL_ID, class: "public-catalog-columns__option",
                        input {
                            id: ALL_ID,
                            r#type: "checkbox",
                            checked: all,
                            // 目录至少保留一列，所以“全选”只有全部与部分两种状态；部分可见是混合态。
                            // 原生 indeterminate 只能由脚本设置，这里用 aria-checked 表达并由样式绘制。
                            aria_checked: if all { "true" } else { "mixed" },
                            onchange: move |event| {
                                let current = visibility();
                                // 勾选显示当前布局的全部列；取消时只保留装备列。
                                let next = if event.checked() {
                                    current.with_all_columns(layout)
                                } else {
                                    current.with_only_item(layout)
                                };
                                if next != current {
                                    visibility.set(next);
                                    next.save();
                                }
                            },
                        }
                        span { {t!("catalog-columns-all")} }
                    }
                    for &(column, _) in layout.columns() {
                        label {
                            key: "{column.input_id()}",
                            r#for: column.input_id(),
                            class: "public-catalog-columns__option",
                            input {
                                id: column.input_id(),
                                r#type: "checkbox",
                                checked: visible.contains(column),
                                disabled: last_column && visible.contains(column),
                                onchange: move |event| {
                                    let current = visibility();
                                    let next = current.with_column(layout, column, event.checked());
                                    if next != current {
                                        visibility.set(next);
                                        next.save();
                                    }
                                },
                            }
                            span { {column.label(layout)} }
                        }
                    }
                }
            } else {
                // 折叠时保留 aria-controls 目标，但不渲染不可操作的字段和译文。
                div { id: PANEL_ID, hidden: true }
            }
        }
    }
}

/// 列表图标；1.5px stroke 与目录行操作图标一致。
#[component]
fn ColumnsIcon() -> Element {
    rsx! {
        svg {
            class: "public-catalog-columns__icon",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.5",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            path { d: "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01" }
        }
    }
}

impl CatalogColumn {
    fn input_id(self) -> &'static str {
        match self {
            Self::Code => "public-catalog-column-code",
            Self::Item => "public-catalog-column-item",
            Self::Group => "public-catalog-column-group",
            Self::AverageRoll => "public-catalog-column-average-roll",
            Self::Level => "public-catalog-column-level",
            Self::Attributes => "public-catalog-column-attributes",
            Self::Seller => "public-catalog-column-seller",
            Self::Price => "public-catalog-column-price",
            Self::NextBid => "public-catalog-column-next-bid",
            Self::Leader => "public-catalog-column-leader",
            Self::Remaining => "public-catalog-column-remaining",
            Self::Status => "public-catalog-column-status",
            Self::Actions => "public-catalog-column-actions",
        }
    }

    fn label(self, layout: CatalogLayout) -> String {
        match self {
            Self::Code => t!("catalog-code"),
            Self::Item => t!("catalog-item"),
            Self::Group => t!("catalog-group"),
            Self::AverageRoll => t!("equipment-avg-roll"),
            Self::Level => t!("catalog-level"),
            Self::Attributes => t!("catalog-primary-attributes"),
            Self::Seller => t!("catalog-seller"),
            Self::Price if layout == CatalogLayout::Preview => t!("catalog-start-price"),
            Self::Price => t!("catalog-current"),
            Self::NextBid => t!("catalog-next-bid"),
            Self::Leader => t!("catalog-leader"),
            Self::Remaining => t!("catalog-remaining"),
            Self::Status => t!("catalog-status"),
            Self::Actions => t!("catalog-actions"),
        }
    }
}

fn close(mut open: Signal<bool>) {
    open.set(false);
    focus_panel_trigger(TOGGLE_ID);
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::rc::Rc;

    use dioxus::core::{AttributeValue, Mutation};
    use dioxus::prelude::*;

    use super::{CatalogColumnsMenu, CatalogLayout, CatalogVisibility, TOGGLE_ID};
    use crate::api::PublicSession;
    use crate::surface::panel_escape::PanelBrowser;
    use crate::surface::session::{SessionCommandScope, SessionMenu};
    use crate::surface::{
        PublicSessionResource, dispatch_page_event, page_event_target, page_test_contexts,
        rendered_event_target, run_page_events, settle_page,
    };

    fn menu() -> Element {
        page_test_contexts();
        let visibility = use_signal(CatalogVisibility::default);
        rsx! { CatalogColumnsMenu { visibility, layout: CatalogLayout::Live } }
    }

    #[derive(Clone, PartialEq, Routable)]
    enum MenuFixtureRoute {
        #[route("/")]
        Menus {},
    }

    fn menus() -> Element {
        rsx! { Router::<MenuFixtureRoute> {} }
    }

    #[component]
    fn Menus() -> Element {
        page_test_contexts();
        let visibility = use_signal(CatalogVisibility::default);
        let session: PublicSessionResource = use_resource(|| async {
            Ok(Some(PublicSession {
                authenticated: true,
                user_id: "fixture-user".to_owned(),
                username: "Player".to_owned(),
                expires_at: None,
                absolute_expires_at: None,
            }))
        });
        use_context_provider(|| session);
        use_context_provider(|| SessionCommandScope(ScopeId::APP));
        rsx! { CatalogColumnsMenu { visibility, layout: CatalogLayout::Live } SessionMenu {} }
    }

    #[test]
    fn escape_closes_the_focused_open_panel_or_the_last_one_opened() {
        const ACCOUNT: &str = "public-account-session-trigger";
        const ACCOUNT_PANEL: &str = "public-account-session-panel";
        run_page_events(
            module_path!(),
            "escape_closes_the_focused_open_panel_or_the_last_one_opened",
            || {
                for (open_order, focus, dismissed) in [
                    ([Some(ACCOUNT), None], None, ACCOUNT),
                    ([Some(TOGGLE_ID), Some(ACCOUNT)], Some(ACCOUNT), ACCOUNT),
                    ([Some(ACCOUNT), Some(TOGGLE_ID)], Some(ACCOUNT), ACCOUNT),
                    ([Some(ACCOUNT), Some(TOGGLE_ID)], Some(ACCOUNT_PANEL), ACCOUNT),
                    ([Some(TOGGLE_ID), Some(ACCOUNT)], Some(TOGGLE_ID), TOGGLE_ID),
                    ([Some(TOGGLE_ID), Some(ACCOUNT)], None, ACCOUNT),
                    ([Some(ACCOUNT), Some(TOGGLE_ID)], None, TOGGLE_ID),
                ] {
                    let browser = Rc::new(PanelBrowser::default());
                    let mut dom = VirtualDom::new(menus).with_root_context(browser.clone());
                    dom.rebuild_in_place();
                    settle_page(&mut dom);
                    let toggle = |id| rendered_event_target(&dom, "button", "id", id, "onclick");
                    let targets: Vec<_> =
                        open_order.into_iter().flatten().map(|id| (id, toggle(id))).collect();
                    for &(_, target) in &targets {
                        dispatch_page_event(&dom, "click", target, "");
                        settle_page(&mut dom);
                    }
                    if focus == Some(ACCOUNT_PANEL) {
                        browser.focused_panel.set(focus);
                    } else {
                        browser.focus.set(focus);
                    }
                    let escape = browser.escape.borrow().clone().expect("展开时监听文档 Escape");
                    browser.modal_open.set(true);
                    assert!(!escape(), "前景模态先处理 Escape");
                    browser.modal_open.set(false);
                    assert!(escape());
                    let edits = settle_page(&mut dom);
                    for (id, target) in targets {
                        assert_eq!(edits.iter().any(|edit| matches!(edit,
                        Mutation::SetAttribute { id, name: "aria-expanded", value: AttributeValue::Text(value), .. }
                            if *id == target && value == "false"
                    )), id == dismissed);
                    }
                    assert_eq!(browser.focus.get(), Some(dismissed));
                    drop(dom);
                    assert!(browser.escape.borrow().is_none());
                }
            },
        );
    }

    #[test]
    fn outside_escape_closes_the_open_menu_and_restores_the_toggle() {
        run_page_events(
            module_path!(),
            "outside_escape_closes_the_open_menu_and_restores_the_toggle",
            || {
                let browser = Rc::new(PanelBrowser::default());
                let mut dom = VirtualDom::new(menu).with_root_context(Rc::clone(&browser));
                let initial = dom.rebuild_to_vec().edits;
                settle_page(&mut dom);
                assert!(browser.escape.borrow().is_none());
                let toggle = page_event_target(&initial, "click", "aria-expanded", "false");
                dispatch_page_event(&dom, "click", toggle, "");
                settle_page(&mut dom);

                // 文档事件不经过面板或入口；替身只转发当前安装的监听器。
                let escape = browser.escape.borrow().clone().expect("面板展开时监听文档 Escape");
                assert!(escape());
                let closed = settle_page(&mut dom);
                assert!(closed.iter().any(|edit| matches!(edit,
                Mutation::SetAttribute { id, name: "aria-expanded", value: AttributeValue::Text(value), .. }
                    if *id == toggle && value == "false"
            )));
                assert_eq!(browser.focus.get(), Some(TOGGLE_ID));
                assert!(browser.escape.borrow().is_none());
                assert!(!escape());
                dispatch_page_event(&dom, "click", toggle, "");
                settle_page(&mut dom);
                assert!(browser.escape.borrow().is_some());
                drop(dom);
                assert!(browser.escape.borrow().is_none());
            },
        );
    }
}
