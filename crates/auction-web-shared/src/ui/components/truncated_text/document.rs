//! 截断提示的模态归属、焦点目标与文档操作。

#[cfg(not(target_arch = "wasm32"))]
use std::rc::Rc;

#[cfg(not(target_arch = "wasm32"))]
use dioxus::prelude::try_consume_context;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

#[cfg(not(target_arch = "wasm32"))]
use super::host_document::TooltipBrowserFixture;
use super::{TooltipPosition, TooltipRequest};
use crate::a11y::{
    ARIA_MODAL_SELECTOR, EscapeUnsubscribe, FOCUSABLE_SELECTOR, with_native_modal_selector,
};

#[cfg(not(target_arch = "wasm32"))]
pub(super) use super::host_document::{DomElement as Element, DomPortal as PortalElement};
#[cfg(target_arch = "wasm32")]
pub(super) use web_sys::{Document, Element, HtmlElement as PortalElement};
#[cfg(not(target_arch = "wasm32"))]
pub(super) type Document = Rc<TooltipBrowserFixture>;

pub(super) fn current() -> Option<Document> {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()?.document()
    }
    #[cfg(not(target_arch = "wasm32"))]
    try_consume_context()
}

pub(super) fn top_dialog(document: &Document) -> Option<Element> {
    #[cfg(target_arch = "wasm32")]
    {
        crate::a11y::top_modal(document)
    }
    #[cfg(not(target_arch = "wasm32"))]
    crate::a11y::top_modal_with(
        |selector| -> Result<_, ()> {
            Ok(document.last_matching(None, selector)?.map(|last| {
                document
                    .active_element()
                    .and_then(|focus| focus.closest(selector).ok().flatten())
                    .unwrap_or(last)
            }))
        },
        |native| document.last_matching(native, ARIA_MODAL_SELECTOR).ok().flatten(),
    )
}

pub(super) fn contains(parent: &Element, element: &Element) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        parent.contains(Some(element.as_ref()))
    }
    #[cfg(not(target_arch = "wasm32"))]
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
    #[cfg(not(target_arch = "wasm32"))]
    _document.focus(&element);
}

pub(super) fn portal_parent(document: &Document, trigger: &Element) -> Option<Element> {
    // 保持浮层在触发元素所属的模态内，避免 portal 落到顶层或遮罩之后。
    with_native_modal_selector(|selector| {
        trigger.closest(&format!("{selector}, {ARIA_MODAL_SELECTOR}"))
    })
    .ok()
    .flatten()
    .or_else(|| body(document))
}

fn body(document: &Document) -> Option<Element> {
    #[cfg(target_arch = "wasm32")]
    {
        document.body().map(Into::into)
    }
    #[cfg(not(target_arch = "wasm32"))]
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
    #[cfg(not(target_arch = "wasm32"))]
    Some(document.append_portal(parent, &request.tooltip_id, &request.value))
}

pub(super) fn remove_portal(element: PortalElement) {
    #[cfg(target_arch = "wasm32")]
    if let Some(parent) = element.parent_node() {
        let _ = parent.remove_child(&element);
    }
    #[cfg(not(target_arch = "wasm32"))]
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
    #[cfg(not(target_arch = "wasm32"))]
    Some(document.listen_escape(on_escape))
}

pub(super) fn anchor_left_top_bottom(element: &Element) -> (f64, f64, f64) {
    #[cfg(target_arch = "wasm32")]
    {
        let rectangle = element.get_bounding_client_rect();
        (rectangle.left(), rectangle.top(), rectangle.bottom())
    }
    #[cfg(not(target_arch = "wasm32"))]
    element.bounding_box()
}
