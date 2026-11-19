//! 截断文本浮层的定位、显示与关闭；浮层留在触发元素所属的模态或正文中。

#[cfg(any(target_arch = "wasm32", test))]
use std::cell::Cell;

use dioxus::prelude::*;

use super::document::{self, Element, PortalElement};
use super::interaction::TooltipInteraction;
#[cfg(any(target_arch = "wasm32", test))]
use super::tracking::AnchorFrames;
use super::{TooltipPosition, TooltipRequest};

type Viewport = (f64, f64, f64, f64);

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

    let rectangle = document::anchor_rectangle(&element);
    let position = position_tooltip(rectangle, tooltip_viewport_bounds());
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
    #[cfg(any(target_arch = "wasm32", test))]
    track_anchor(document, element, rectangle, state, request.tooltip_id.clone());
}

#[cfg(any(target_arch = "wasm32", test))]
fn track_anchor(
    document: document::Document,
    element: Element,
    rectangle: [f64; 4],
    mut state: TooltipInteraction,
    tooltip_id: String,
) {
    let previous_parent = document::parent_element(&element);
    let applied = Cell::new(rectangle);
    let checked = Cell::new(rectangle);
    state.stop_anchor_tracking();
    let frames = AnchorFrames::start(document, move |document| {
        if !document::is_connected(document, &element) {
            // 锚点移除后，仅仍连接的原入口容器可以承接浮层焦点。
            let parent =
                previous_parent.as_ref().filter(|parent| document::is_connected(document, parent));
            state.dismiss(Some(document), &tooltip_id, parent);
            return;
        }
        let rectangle = document::anchor_rectangle(&element);
        let viewport = tooltip_viewport_bounds();
        let visible = intersect_viewport(rectangle, viewport);
        if !has_area(rectangle) || !has_area(visible) {
            state.dismiss(Some(document), &tooltip_id, None);
            return;
        }
        // 相对实际采用的矩形比较，逐帧的小位移也能累积，半像素边界的抖动不会翻边。
        if rectangle == checked.get()
            || rectangle
                .iter()
                .zip(applied.get())
                .all(|(current, previous)| (current - previous).abs() < 0.5)
        {
            return;
        }
        if !visible_through_ancestors(&element, visible) {
            state.dismiss(Some(document), &tooltip_id, None);
            return;
        }
        // 定位结果相同时保持应用基准，同时避免静止帧重复读取祖先样式。
        checked.set(rectangle);
        let position = position_tooltip(rectangle, viewport);
        if *state.position.peek() != Some(position) {
            if let Some(popup) = state.portal.peek().as_ref() {
                document::position_portal(popup, position);
            }
            let mut position_signal = state.position;
            position_signal.set(Some(position));
            applied.set(rectangle);
        }
    });
    *state.anchor_frames.write() = frames;
}

#[cfg(any(target_arch = "wasm32", test))]
fn intersect_viewport([left, top, right, bottom]: [f64; 4], viewport: Viewport) -> [f64; 4] {
    let (viewport_left, viewport_top, viewport_width, viewport_height) = viewport;
    [
        left.max(viewport_left),
        top.max(viewport_top),
        right.min(viewport_left + viewport_width),
        bottom.min(viewport_top + viewport_height),
    ]
}

#[cfg(any(target_arch = "wasm32", test))]
fn has_area([left, top, right, bottom]: [f64; 4]) -> bool {
    right > left && bottom > top
}

#[cfg(any(target_arch = "wasm32", test))]
fn visible_through_ancestors(element: &Element, mut visible: [f64; 4]) -> bool {
    let mut parent = document::parent_element(element);
    while let Some(ancestor) = parent {
        let Some((clip_x, clip_y)) = document::clipping_axes(&ancestor) else {
            return false;
        };
        if clip_x || clip_y {
            let [left, top, right, bottom] = document::anchor_rectangle(&ancestor);
            if clip_x {
                visible[0] = visible[0].max(left);
                visible[2] = visible[2].min(right);
            }
            if clip_y {
                visible[1] = visible[1].max(top);
                visible[3] = visible[3].min(bottom);
            }
            if !has_area(visible) {
                return false;
            }
        }
        parent = document::parent_element(&ancestor);
    }
    true
}

fn position_tooltip([left, top, _, bottom]: [f64; 4], viewport: Viewport) -> TooltipPosition {
    let (viewport_left, viewport_top, viewport_width, viewport_height) = viewport;
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
    TooltipPosition {
        left: left.clamp(minimum_left, maximum_left),
        top: if above { above_top } else { below_top },
        width,
        maximum_height: if above { available_above } else { available_below },
        placement: if above { "above" } else { "below" },
    }
}

/// 返回相对布局视口的可见区域；旧浏览器回退到完整窗口，不限制用户缩放。
#[cfg(target_arch = "wasm32")]
fn tooltip_viewport_bounds() -> Viewport {
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
fn tooltip_viewport_bounds() -> Viewport {
    (0.0, 0.0, 1024.0, 768.0)
}

pub(super) fn remove_tooltip(mut portal_element: Signal<Option<PortalElement>>) {
    let element = portal_element.peek().clone();
    if let Some(element) = element {
        portal_element.set(None);
        document::remove_portal(element);
    }
}
