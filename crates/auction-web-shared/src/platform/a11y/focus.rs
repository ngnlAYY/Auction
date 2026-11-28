//! 查询对话框内可聚焦控件并选择初始焦点。

use wasm_bindgen::JsCast;
use web_sys::{Document, Element, HtmlElement};

use super::FOCUSABLE_SELECTOR;

pub(super) fn current_focusable(document: &Document) -> Option<HtmlElement> {
    let element = document.active_element()?.dyn_into::<HtmlElement>().ok()?;
    is_focusable(&element).then_some(element)
}

pub(super) fn is_focusable(element: &HtmlElement) -> bool {
    element.matches(FOCUSABLE_SELECTOR).unwrap_or(false)
        && !element.matches(":disabled").unwrap_or(false)
        && is_visible(element)
}

/// 已显示且仍连接的节点；隐藏祖先不参与焦点循环，也不承接程序化焦点。
pub(super) fn is_visible(element: &HtmlElement) -> bool {
    element.is_connected()
        // 手机折叠菜单、关闭的 details 与隐藏祖先不能成为焦点循环的首尾控件。
        && element.closest("[hidden], [inert], [aria-hidden=\"true\"]").ok().flatten().is_none()
        && element.get_client_rects().length() > 0
        && web_sys::window().and_then(|window| window.get_computed_style(element).ok().flatten())
            .is_none_or(|style| !matches!(style.get_property_value("visibility").ok().as_deref(), Some("hidden" | "collapse")))
}

/// 模态内可保留的焦点目标：可聚焦控件，或显式 `tabindex`（含 -1）的容器。
/// 校验后聚焦的摘要容器不在 Tab 顺序中，但它的焦点不应被下一次无关变更抢走。
pub(super) fn is_focus_target(element: &HtmlElement) -> bool {
    is_focusable(element) || (element.has_attribute("tabindex") && is_visible(element))
}

pub(super) fn preferred_dialog_focus(dialog: &Element) -> Option<HtmlElement> {
    if let Some(target) = dialog
        .query_selector("[autofocus]")
        .ok()
        .flatten()
        .and_then(|element| element.dyn_into::<HtmlElement>().ok())
        .filter(is_focusable)
    {
        return Some(target);
    }

    let nodes = dialog.query_selector_all(FOCUSABLE_SELECTOR).ok()?;
    for index in 0..nodes.length() {
        let Some(node) = nodes.item(index) else {
            continue;
        };
        let Ok(control) = node.dyn_into::<HtmlElement>() else {
            continue;
        };
        if is_focusable(&control) {
            return Some(control);
        }
    }
    let panel = dialog.clone().dyn_into::<HtmlElement>().ok()?;
    if !panel.has_attribute("tabindex") {
        panel.set_tab_index(-1);
    }
    Some(panel)
}
