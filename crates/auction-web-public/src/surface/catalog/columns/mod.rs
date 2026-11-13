//! 目录列设置：类型工具栏右端的图标按钮与下拉面板（标题、全选、逐列勾选）。
//! 只改写本地列偏好，不改变目录查询、URL 或竞价状态；隐藏列不卸载表格命令宿主。

mod state;

pub(crate) use state::{CatalogColumn, CatalogLayout, CatalogVisibility};

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

const TOGGLE_ID: &str = "public-catalog-columns-toggle";
const PANEL_ID: &str = "public-catalog-columns-panel";
const TITLE_ID: &str = "public-catalog-columns-title";
const HINT_ID: &str = "public-catalog-columns-hint";
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

    rsx! {
        div { class: "public-catalog-columns-menu",
            button {
                id: TOGGLE_ID,
                class: "public-catalog-columns__toggle",
                r#type: "button",
                aria_haspopup: "true",
                aria_controls: PANEL_ID,
                aria_expanded: if open() { "true" } else { "false" },
                // 图标按钮：可访问名称与悬停提示都是“列设置”。
                aria_label: t!("catalog-columns-settings"),
                title: t!("catalog-columns-settings"),
                onclick: move |_| toggle.set(!toggle()),
                onkeydown: move |event| {
                    if open() && event.key() == Key::Escape {
                        event.prevent_default();
                        event.stop_propagation();
                        close(open);
                    }
                },
                ColumnsIcon {}
            }
            if open() {
                // 透明遮罩接住面板外的点击。
                div { class: "public-catalog-columns__backdrop", onclick: move |_| close(open) }
                div {
                    id: PANEL_ID,
                    class: "public-catalog-columns",
                    role: "group",
                    aria_labelledby: TITLE_ID,
                    aria_describedby: HINT_ID,
                    onkeydown: move |event| {
                        if event.key() == Key::Escape {
                            event.prevent_default();
                            event.stop_propagation();
                            close(open);
                        }
                    },
                    strong { id: TITLE_ID, class: "public-catalog-columns__title", {t!("catalog-columns")} }
                    p { id: HINT_ID, class: "public-visually-hidden", {t!("catalog-columns-hint")} }
                    label { r#for: ALL_ID, class: "public-catalog-columns__option",
                        input {
                            id: ALL_ID,
                            r#type: "checkbox",
                            checked: all,
                            // 目录至少保留一列，所以“全选”只有全部与部分两种状态；部分可见是混合态。
                            // 原生 indeterminate 只能由脚本设置，这里用 aria-checked 表达并由样式绘制。
                            aria_checked: if all { "true" } else { "mixed" },
                            aria_describedby: HINT_ID,
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
                                aria_describedby: HINT_ID,
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
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;

        if let Some(button) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id(TOGGLE_ID))
            .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = button.focus();
        }
    }
}
