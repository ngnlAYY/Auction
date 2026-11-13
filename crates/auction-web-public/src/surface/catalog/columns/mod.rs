//! 目录列设置沿用公开页内联展开面板，不引入弹层或改变表格命令宿主。

mod state;

pub(crate) use state::{CatalogColumn, CatalogLayout, CatalogVisibility};

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

const TOGGLE_ID: &str = "public-catalog-columns-toggle";
const PANEL_ID: &str = "public-catalog-columns-panel";
const TITLE_ID: &str = "public-catalog-columns-title";
const HINT_ID: &str = "public-catalog-columns-hint";

#[component]
pub(crate) fn CatalogColumnsButton(mut open: Signal<bool>) -> Element {
    rsx! {
        button {
            id: TOGGLE_ID,
            class: "public-catalog-columns__toggle",
            r#type: "button",
            aria_controls: PANEL_ID,
            aria_expanded: if open() { "true" } else { "false" },
            onclick: move |_| open.set(!open()),
            onkeydown: move |event| {
                if open() && event.key() == Key::Escape {
                    event.prevent_default();
                    event.stop_propagation();
                    close(open);
                }
            },
            {t!("catalog-columns-settings")}
        }
    }
}

#[component]
pub(crate) fn CatalogColumnsPanel(
    open: Signal<bool>,
    mut visibility: Signal<CatalogVisibility>,
    layout: CatalogLayout,
) -> Element {
    if !open() {
        // 折叠时保留 aria-controls 目标，但不渲染不可操作的字段和译文。
        return rsx! { div { id: PANEL_ID, hidden: true } };
    }
    let visible = visibility().for_layout(layout);
    let last_column = visible.visible_count(layout) == 1;

    rsx! {
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
            div { class: "public-catalog-columns__header",
                strong { id: TITLE_ID, {t!("catalog-columns")} }
                div { class: "public-catalog-columns__actions",
                    button {
                        class: "public-catalog-columns__reset",
                        r#type: "button",
                        onclick: move |_| {
                            // 默认可见集刻意隐藏装备分组，恢复全部列必须显式打开当前布局的所有位。
                            let all = visibility().with_all_columns(layout);
                            visibility.set(all);
                            all.save();
                        },
                        {t!("catalog-columns-reset")}
                    }
                    button {
                        class: "public-catalog-columns__close",
                        r#type: "button",
                        aria_label: t!("catalog-columns-close"),
                        onclick: move |_| close(open),
                        {t!("catalog-close")}
                    }
                }
            }
            p { id: HINT_ID, class: "public-catalog-columns__hint",
                {t!("catalog-columns-hint")}
            }
            div { class: "public-catalog-columns__grid",
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
