//! 截断文本浮层的定位、显示与关闭；浮层留在触发元素所属的模态或正文中。

use dioxus::prelude::*;

use super::document::{self, Element, PortalElement};
use super::interaction::TooltipInteraction;
use super::{TooltipPosition, TooltipRequest};

pub(super) fn update_truncated_state(
    element: &Element,
    display_differs: bool,
    mut truncated: Signal<bool>,
    state: TooltipInteraction,
    tooltip_id: &str,
) -> bool {
    let next = element_is_truncated(element, display_differs);
    if !next {
        // 先归还焦点，随后才能移除入口的 tabindex。
        state.close_programmatically(tooltip_id);
    }
    // 相同测量值不写信号，避免表格中每个文本在窗口与观察器重复通知时重渲染。
    let changed = *truncated.peek() != next;
    if changed {
        truncated.set(next);
    }
    next
}

fn element_is_truncated(element: &Element, display_differs: bool) -> bool {
    display_differs || element.scroll_width() > element.client_width()
}

pub(super) fn show_tooltip(
    display_differs: bool,
    truncated: Signal<bool>,
    mut state: TooltipInteraction,
    request: &TooltipRequest,
) {
    let Some(document) = document::current() else {
        return;
    };
    let Some(element) = document.get_element_by_id(&request.trigger_id) else {
        return;
    };
    if !update_truncated_state(&element, display_differs, truncated, state, &request.tooltip_id) {
        return;
    }

    let (left, top, bottom) = document::anchor_left_top_bottom(&element);
    let (viewport_left, viewport_top, viewport_width, viewport_height) = tooltip_viewport_bounds();
    let width = 360.0_f64.min((viewport_width - 16.0).max(1.0));
    let gap = 8.0;
    let margin = 8.0;
    let minimum_top = viewport_top + margin;
    let maximum_top = (viewport_top + viewport_height - margin).max(minimum_top);
    let above_top = (top - gap).clamp(minimum_top, maximum_top);
    let below_top = (bottom + gap).clamp(minimum_top, maximum_top);
    let available_above = (above_top - minimum_top).max(1.0);
    let available_below = (maximum_top - below_top).max(1.0);
    let above = available_above > available_below;
    let minimum_left = viewport_left + margin;
    let maximum_left = (viewport_left + viewport_width - width - margin).max(minimum_left);
    let position = TooltipPosition {
        left: left.clamp(minimum_left, maximum_left),
        top: if above { above_top } else { below_top },
        width,
        maximum_height: if above { available_above } else { available_below },
        placement: if above { "above" } else { "below" },
    };
    if request.portal {
        let Some(parent) = document::portal_parent(&document, &element) else { return };
        let previous = state.portal.peek().clone();
        let Some(tooltip) = document::mount_portal(&document, &parent, previous, request, position)
        else {
            return;
        };
        state.portal.set(Some(tooltip));
    }
    state.position.set(Some(position));
}

/// 返回相对布局视口的可见区域；旧浏览器回退到完整窗口，不限制用户缩放。
#[cfg(target_arch = "wasm32")]
fn tooltip_viewport_bounds() -> (f64, f64, f64, f64) {
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

#[cfg(not(target_arch = "wasm32"))]
fn tooltip_viewport_bounds() -> (f64, f64, f64, f64) {
    (0.0, 0.0, 1024.0, 768.0)
}

pub(super) fn remove_tooltip(mut portal_element: Signal<Option<PortalElement>>) {
    let element = portal_element.peek().clone();
    if let Some(element) = element {
        portal_element.set(None);
        document::remove_portal(element);
    }
}
