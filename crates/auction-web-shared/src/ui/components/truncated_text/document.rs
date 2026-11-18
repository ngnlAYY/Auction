//! 截断提示的模态归属、焦点目标与文档操作。

#[cfg(all(test, not(target_arch = "wasm32")))]
use std::rc::Rc;

#[cfg(all(test, not(target_arch = "wasm32")))]
use dioxus::prelude::try_consume_context;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue};

#[cfg(all(test, not(target_arch = "wasm32")))]
use super::tests::Browser;
use super::{TooltipPosition, TooltipRequest};
use crate::a11y::{ARIA_MODAL_SELECTOR, EscapeUnsubscribe, FOCUSABLE_SELECTOR};

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(super) use super::tests::{DomElement as Element, DomPortal as PortalElement};
#[cfg(target_arch = "wasm32")]
pub(super) use web_sys::{Document, Element, HtmlElement as PortalElement};
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(super) type Document = Rc<Browser>;

const NATIVE_MODAL_SELECTOR: &str = "dialog:modal";
const OPEN_DIALOG_SELECTOR: &str = "dialog[open]";

pub(super) fn current() -> Option<Document> {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()?.document()
    }
    #[cfg(all(test, not(target_arch = "wasm32")))]
    try_consume_context()
}

/// 原生顶层模态优先。多层原生对话框优先选取持有焦点的模态，DOM 顺序不一定是打开顺序。
pub(super) fn top_dialog(document: &Document) -> Option<Element> {
    let native = |selector| -> Result<Option<Element>, SelectorError> {
        Ok(last_matching(document, selector)?.map(|last| {
            document
                .active_element()
                .and_then(|focus| focus.closest(selector).ok().flatten())
                .unwrap_or(last)
        }))
    };
    // 空匹配表示支持 :modal 且没有原生模态；只在选择器不可用时把 open 对话框视为模态。
    native(NATIVE_MODAL_SELECTOR)
        .or_else(|_error| native(OPEN_DIALOG_SELECTOR))
        .ok()
        .flatten()
        .or_else(|| last_matching(document, ARIA_MODAL_SELECTOR).ok().flatten())
}

#[cfg(target_arch = "wasm32")]
type SelectorError = JsValue;
#[cfg(all(test, not(target_arch = "wasm32")))]
type SelectorError = ();

fn last_matching(document: &Document, selector: &str) -> Result<Option<Element>, SelectorError> {
    #[cfg(target_arch = "wasm32")]
    {
        let nodes = document.query_selector_all(selector)?;
        Ok(nodes
            .length()
            .checked_sub(1)
            .and_then(|last| nodes.item(last))
            .and_then(|node| node.dyn_into().ok()))
    }
    #[cfg(all(test, not(target_arch = "wasm32")))]
    document.last_matching(selector)
}

pub(super) fn contains(parent: &Element, element: &Element) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        parent.contains(Some(element.as_ref()))
    }
    #[cfg(all(test, not(target_arch = "wasm32")))]
    parent.contains(element)
}

pub(super) fn focus_trigger(document: &Document, trigger: &Element) {
    // 链接里的截断文本不在 Tab 顺序中，最近的可聚焦祖先承接焦点。
    let Ok(Some(control)) = trigger.closest(FOCUSABLE_SELECTOR) else { return };
    focus(document, control);
}

fn focus(_document: &Document, element: Element) {
    #[cfg(target_arch = "wasm32")]
    crate::a11y::focus_control(element);
    #[cfg(all(test, not(target_arch = "wasm32")))]
    _document.focus(&element);
}

pub(super) fn portal_parent(document: &Document, trigger: &Element) -> Option<Element> {
    // 保持浮层在触发元素所属的模态内，避免 portal 落到顶层或遮罩之后。
    trigger
        .closest(NATIVE_MODAL_SELECTOR)
        .or_else(|_error| trigger.closest(OPEN_DIALOG_SELECTOR))
        .ok()
        .flatten()
        .or_else(|| trigger.closest(ARIA_MODAL_SELECTOR).ok().flatten())
        .or_else(|| body(document))
}

fn body(document: &Document) -> Option<Element> {
    #[cfg(target_arch = "wasm32")]
    {
        document.body().map(Into::into)
    }
    #[cfg(all(test, not(target_arch = "wasm32")))]
    document.body()
}

pub(super) fn mount_portal(
    document: &Document,
    parent: &Element,
    _previous: Option<PortalElement>,
    request: &TooltipRequest,
    _position: TooltipPosition,
) -> Option<PortalElement> {
    #[cfg(target_arch = "wasm32")]
    {
        let tooltip = match _previous {
            Some(tooltip) => tooltip,
            None => document.create_element("span").ok()?.dyn_into::<PortalElement>().ok()?,
        };
        if !tooltip.parent_node().is_some_and(|current| current.is_same_node(Some(parent.as_ref())))
        {
            parent.append_child(&tooltip).ok()?;
        }
        let _ = tooltip.set_attribute("id", &request.tooltip_id);
        let _ = tooltip.set_attribute("class", &request.class_name);
        let _ = tooltip.set_attribute("role", "tooltip");
        let _ = tooltip.set_attribute("tabindex", "0");
        let _ = tooltip.set_attribute("data-placement", _position.placement);
        tooltip.set_text_content(Some(&request.value));
        let style = tooltip.style();
        for (property, value) in [
            ("--truncated-text-tooltip-left", _position.left),
            ("--truncated-text-tooltip-top", _position.top),
            ("--truncated-text-tooltip-width", _position.width),
            ("--truncated-text-tooltip-max-height", _position.maximum_height),
        ] {
            let _ = style.set_property(property, &format!("{value}px"));
        }
        Some(tooltip)
    }
    #[cfg(all(test, not(target_arch = "wasm32")))]
    Some(document.append_portal(parent, &request.tooltip_id))
}

pub(super) fn remove_portal(element: PortalElement) {
    #[cfg(target_arch = "wasm32")]
    if let Some(parent) = element.parent_node() {
        let _ = parent.remove_child(&element);
    }
    #[cfg(all(test, not(target_arch = "wasm32")))]
    element.remove();
}

pub(super) fn listen_escape(
    document: Document,
    on_escape: impl Fn() -> bool + 'static,
) -> Option<EscapeUnsubscribe> {
    #[cfg(target_arch = "wasm32")]
    {
        crate::a11y::bind_document_escape(document, true, on_escape)
    }
    #[cfg(all(test, not(target_arch = "wasm32")))]
    Some(document.listen_escape(on_escape))
}

pub(super) fn anchor_left_top_bottom(element: &Element) -> (f64, f64, f64) {
    #[cfg(target_arch = "wasm32")]
    {
        let rectangle = element.get_bounding_client_rect();
        (rectangle.left(), rectangle.top(), rectangle.bottom())
    }
    #[cfg(all(test, not(target_arch = "wasm32")))]
    element.bounding_box()
}
