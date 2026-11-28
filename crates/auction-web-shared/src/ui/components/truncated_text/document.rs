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
    cfg_select! {
        target_arch = "wasm32" => web_sys::window()?.document(),
        _ => try_consume_context(),
    }
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
    cfg_select! {
        target_arch = "wasm32" => parent.contains(Some(element.as_ref())),
        _ => parent.contains(element),
    }
}

pub(super) fn focus_trigger(document: &Document, trigger: &Element) {
    // 链接里的截断文本不在 Tab 顺序中，最近的可聚焦祖先承接焦点。
    let Ok(Some(control)) = trigger.closest(FOCUSABLE_SELECTOR) else { return };
    focus(document, control);
}

fn focus(_document: &Document, element: Element) {
    cfg_select! {
        target_arch = "wasm32" => { crate::a11y::focus_control(element); }
        _ => { _document.focus(&element); }
    }
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
    cfg_select! {
        target_arch = "wasm32" => document.body().map(Into::into),
        _ => document.body(),
    }
}

pub(super) fn mount_portal(
    document: &Document,
    parent: &Element,
    _previous: Option<PortalElement>,
    request: &TooltipRequest,
    _position: TooltipPosition,
) -> Option<PortalElement> {
    #[cfg(target_arch = "wasm32")]
    let tooltip = {
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
        tooltip.set_text_content(Some(&request.value));
        tooltip
    };
    // 与 wasm 分支一致：同一 ID 的浮层原地复用，悬停后再聚焦不能创建第二个节点。
    #[cfg(not(target_arch = "wasm32"))]
    let tooltip = match _previous {
        Some(previous) if document.refresh_portal(&previous, parent, &request.value) => previous,
        _ => document.append_portal(parent, &request.tooltip_id, &request.value),
    };
    #[cfg(any(target_arch = "wasm32", test))]
    position_portal(&tooltip, _position);
    Some(tooltip)
}

#[cfg(any(target_arch = "wasm32", test))]
pub(super) fn position_portal(tooltip: &PortalElement, position: TooltipPosition) {
    let _ = tooltip.set_attribute("data-placement", position.placement);
    let style = tooltip.style();
    for (property, value) in [
        ("--truncated-text-tooltip-left", position.left),
        ("--truncated-text-tooltip-top", position.top),
        ("--truncated-text-tooltip-width", position.width),
        ("--truncated-text-tooltip-max-height", position.maximum_height),
    ] {
        let _ = style.set_property(property, &format!("{value}px"));
    }
}

pub(super) fn remove_portal(element: PortalElement) {
    cfg_select! {
        target_arch = "wasm32" => if let Some(parent) = element.parent_node() {
            let _ = parent.remove_child(&element);
        },
        _ => { element.remove(); }
    }
}

pub(super) fn listen_escape(
    document: Document,
    on_escape: impl Fn() -> bool + 'static,
) -> Option<EscapeUnsubscribe> {
    cfg_select! {
        target_arch = "wasm32" => crate::a11y::bind_document_escape(document, true, on_escape),
        _ => Some(document.listen_escape(on_escape)),
    }
}

pub(super) fn anchor_rectangle(element: &Element) -> [f64; 4] {
    cfg_select! {
        target_arch = "wasm32" => {
            let rectangle = element.get_bounding_client_rect();
            [rectangle.left(), rectangle.top(), rectangle.right(), rectangle.bottom()]
        }
        _ => element.bounding_box(),
    }
}

#[cfg(any(target_arch = "wasm32", test))]
pub(super) fn is_connected(_document: &Document, element: &Element) -> bool {
    cfg_select! {
        target_arch = "wasm32" => element.is_connected(),
        _ => _document.get_element_by_id(&element.id)
            .is_some_and(|current| Rc::ptr_eq(&current.rectangle, &element.rectangle)),
    }
}

#[cfg(any(target_arch = "wasm32", test))]
pub(super) fn parent_element(element: &Element) -> Option<Element> {
    cfg_select! {
        target_arch = "wasm32" => element.parent_element(),
        _ => element.parent.as_deref().cloned(),
    }
}

#[cfg(any(target_arch = "wasm32", test))]
pub(super) fn clipping_axes(element: &Element) -> Option<(bool, bool)> {
    #[cfg(target_arch = "wasm32")]
    let (overflow_x, overflow_y) = {
        let style = web_sys::window()?.get_computed_style(element).ok()??;
        (style.get_property_value("overflow-x").ok()?, style.get_property_value("overflow-y").ok()?)
    };
    #[cfg(not(target_arch = "wasm32"))]
    let (overflow_x, overflow_y) = {
        element.overflow_reads.set(element.overflow_reads.get() + 1);
        element.overflow.get()
    };
    Some((overflow_x != "visible", overflow_y != "visible"))
}
