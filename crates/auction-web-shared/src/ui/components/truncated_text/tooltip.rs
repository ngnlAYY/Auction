//! 截断文本浮层的定位、显示与关闭：视口边界内放置，传送门渲染到 body，滚动或缩放时收起。

use dioxus::prelude::*;
use wasm_bindgen::JsCast;

use super::interaction::TooltipInteraction;
use super::{TooltipPosition, TooltipRequest};

pub(super) fn toggle_tooltip(
    mut interaction: TooltipInteraction,
    element: Option<web_sys::Element>,
    display_differs: bool,
    truncated: Signal<bool>,
    request: TooltipRequest,
) {
    if *interaction.pinned.peek() {
        interaction.close();
    } else {
        interaction.dismissed.set(false);
        show_tooltip(
            element,
            display_differs,
            truncated,
            interaction.position,
            interaction.portal,
            request,
        );
        interaction.pinned.set(interaction.position.peek().is_some());
    }
}

pub(super) fn update_truncated_state(
    element: &web_sys::Element,
    display_differs: bool,
    mut truncated: Signal<bool>,
    tooltip_position: Signal<Option<TooltipPosition>>,
    portal_element: Signal<Option<web_sys::HtmlElement>>,
) {
    let next = element_is_truncated(element, display_differs);
    // 相同测量值不写信号，避免表格中每个文本在窗口与观察器重复通知时重渲染。
    let changed = *truncated.peek() != next;
    if changed {
        truncated.set(next);
    }
    if !next {
        hide_tooltip(tooltip_position, portal_element);
    }
}

pub(super) fn element_is_truncated(element: &web_sys::Element, display_differs: bool) -> bool {
    display_differs || element.scroll_width() > element.client_width()
}

pub(super) fn show_tooltip(
    element: Option<web_sys::Element>,
    display_differs: bool,
    mut truncated: Signal<bool>,
    mut tooltip_position: Signal<Option<TooltipPosition>>,
    mut portal_element: Signal<Option<web_sys::HtmlElement>>,
    request: TooltipRequest,
) {
    let Some(element) = element else {
        return;
    };
    let next = element_is_truncated(&element, display_differs);
    let changed = *truncated.peek() != next;
    if changed {
        truncated.set(next);
    }
    if !next {
        hide_tooltip(tooltip_position, portal_element);
        return;
    }

    let rectangle = element.get_bounding_client_rect();
    let (viewport_left, viewport_top, viewport_width, viewport_height) = tooltip_viewport_bounds();
    let width = 360.0_f64.min((viewport_width - 16.0).max(1.0));
    let gap = 8.0;
    let margin = 8.0;
    let minimum_top = viewport_top + margin;
    let maximum_top = (viewport_top + viewport_height - margin).max(minimum_top);
    let above_top = (rectangle.top() - gap).clamp(minimum_top, maximum_top);
    let below_top = (rectangle.bottom() + gap).clamp(minimum_top, maximum_top);
    let available_above = (above_top - minimum_top).max(1.0);
    let available_below = (maximum_top - below_top).max(1.0);
    let above = available_above > available_below;
    let minimum_left = viewport_left + margin;
    let maximum_left = (viewport_left + viewport_width - width - margin).max(minimum_left);
    let position = TooltipPosition {
        left: rectangle.left().clamp(minimum_left, maximum_left),
        top: if above { above_top } else { below_top },
        width,
        maximum_height: if above { available_above } else { available_below },
        placement: if above { "above" } else { "below" },
    };
    if request.portal
        && !render_portal_tooltip(
            &mut portal_element,
            &element,
            &request.value,
            &request.tooltip_id,
            &request.class_name,
            position,
        )
    {
        return;
    }
    tooltip_position.set(Some(position));
}

/// 返回相对布局视口的可见区域；旧浏览器回退到完整窗口，不限制用户缩放。
pub(super) fn tooltip_viewport_bounds() -> (f64, f64, f64, f64) {
    let Some(window) = web_sys::window() else {
        return (0.0, 0.0, 1024.0, 768.0);
    };
    if let Some(viewport) = window.visual_viewport() {
        let (left, top, width, height) =
            (viewport.offset_left(), viewport.offset_top(), viewport.width(), viewport.height());
        if [left, top, width, height].iter().all(|value| value.is_finite())
            && width > 0.0
            && height > 0.0
        {
            return (left, top, width, height);
        }
    }
    let width = window.inner_width().ok().and_then(|value| value.as_f64());
    let height = window.inner_height().ok().and_then(|value| value.as_f64());
    (
        0.0,
        0.0,
        width.filter(|value| value.is_finite() && *value > 0.0).unwrap_or(1024.0),
        height.filter(|value| value.is_finite() && *value > 0.0).unwrap_or(768.0),
    )
}

pub(super) fn hide_tooltip(
    mut tooltip_position: Signal<Option<TooltipPosition>>,
    mut portal_element: Signal<Option<web_sys::HtmlElement>>,
) {
    let visible = tooltip_position.peek().is_some();
    if visible {
        tooltip_position.set(None);
    }
    let element = portal_element.peek().clone();
    if let Some(element) = element {
        if let Some(parent) = element.parent_node() {
            let _ = parent.remove_child(&element);
        }
        portal_element.set(None);
    }
}

pub(super) fn remove_tooltip(mut portal_element: Signal<Option<web_sys::HtmlElement>>) {
    let element = portal_element.peek().clone();
    if let Some(element) = element
        && let Some(parent) = element.parent_node()
    {
        let _ = parent.remove_child(&element);
    }
    portal_element.set(None);
}

pub(super) fn render_portal_tooltip(
    portal_element: &mut Signal<Option<web_sys::HtmlElement>>,
    trigger: &web_sys::Element,
    value: &str,
    tooltip_id: &str,
    tooltip_class_name: &str,
    position: TooltipPosition,
) -> bool {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return false;
    };
    // 弹窗里的浮层留在所属焦点范围与层叠上下文内，避免 body portal 落到遮罩后面。
    let parent = trigger
        .closest("[role=\"dialog\"][aria-modal=\"true\"]")
        .ok()
        .flatten()
        .or_else(|| document.body().map(Into::into));
    let Some(parent) = parent else {
        return false;
    };
    let tooltip = match portal_element() {
        Some(tooltip) => tooltip,
        None => {
            let Ok(element) = document.create_element("span") else {
                return false;
            };
            let Ok(tooltip) = element.dyn_into::<web_sys::HtmlElement>() else {
                return false;
            };
            tooltip
        }
    };
    if !tooltip.parent_node().is_some_and(|current| current.is_same_node(Some(parent.as_ref())))
        && parent.append_child(&tooltip).is_err()
    {
        return false;
    }
    let _ = tooltip.set_attribute("id", tooltip_id);
    let _ = tooltip.set_attribute("class", tooltip_class_name);
    let _ = tooltip.set_attribute("role", "tooltip");
    let _ = tooltip.set_attribute("tabindex", "0");
    let _ = tooltip.set_attribute("data-placement", position.placement);
    tooltip.set_text_content(Some(value));
    let style = tooltip.style();
    let _ = style.set_property("--truncated-text-tooltip-left", &format!("{}px", position.left));
    let _ = style.set_property("--truncated-text-tooltip-top", &format!("{}px", position.top));
    let _ = style.set_property("--truncated-text-tooltip-width", &format!("{}px", position.width));
    let _ = style.set_property(
        "--truncated-text-tooltip-max-height",
        &format!("{}px", position.maximum_height),
    );
    portal_element.set(Some(tooltip));
    true
}
