//! 长文本截断展示与可操作浮层；悬停、聚焦和点击共享同一份完整文本。

mod interaction;

#[cfg(target_arch = "wasm32")]
use interaction::PopupListeners;
use interaction::TooltipInteraction;

use std::sync::atomic::{AtomicUsize, Ordering};

use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
use std::{cell::RefCell, rc::Rc};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

static NEXT_TOOLTIP_ID: AtomicUsize = AtomicUsize::new(1);

#[derive(Clone, Copy, PartialEq)]
struct TooltipPosition {
    left: f64,
    top: f64,
    width: f64,
    maximum_height: f64,
    placement: &'static str,
}

#[cfg(target_arch = "wasm32")]
struct ResizeObserverHandle {
    observer: web_sys::ResizeObserver,
    _callback: Closure<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>,
}

#[cfg(target_arch = "wasm32")]
struct ResizeMeasureHandle {
    window: web_sys::Window,
    callback: Closure<dyn FnMut()>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for ResizeMeasureHandle {
    fn drop(&mut self) {
        let _ = self
            .window
            .remove_event_listener_with_callback("resize", self.callback.as_ref().unchecked_ref());
    }
}

#[cfg(target_arch = "wasm32")]
#[derive(Clone)]
struct TooltipRequest {
    value: String,
    tooltip_id: String,
    class_name: String,
    portal: bool,
}

#[cfg(target_arch = "wasm32")]
struct TooltipDismissHandle {
    window: web_sys::Window,
    resize_callback: Closure<dyn FnMut()>,
    scroll_callback: Closure<dyn FnMut(web_sys::Event)>,
    _popup_listeners: Option<PopupListeners>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for TooltipDismissHandle {
    fn drop(&mut self) {
        let _ = self.window.remove_event_listener_with_callback(
            "resize",
            self.resize_callback.as_ref().unchecked_ref(),
        );
        let _ = self.window.remove_event_listener_with_callback_and_bool(
            "scroll",
            self.scroll_callback.as_ref().unchecked_ref(),
            true,
        );
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for ResizeObserverHandle {
    fn drop(&mut self) {
        self.observer.disconnect();
    }
}

/// 展示可能很长的标识或 metadata，并在确实发生截断时提供可聚焦 tooltip。
///
/// `display` 用于旧版已经提供缩写文本的场景；只要展示值与完整值不同，也视为截断。浏览器端
/// 通过 `ResizeObserver` 重新测量宽度，因此响应式布局变化后仍只把真正需要提示的文本加入
/// Tab 顺序。默认模式保留 `title` 降级；legacy DOM 模式省略该扩展，只输出旧版显式 tooltip。
#[component]
pub fn TruncatedText(
    value: String,
    display: Option<String>,
    size: Option<String>,
    class_name: Option<String>,
    focusable: Option<bool>,
    portal: Option<bool>,
    legacy_dom: Option<bool>,
) -> Element {
    #[cfg(target_arch = "wasm32")]
    let display_differs = display.as_ref().is_some_and(|display| display != &value);
    let display = display.unwrap_or_else(|| value.clone());
    let size = match size.as_deref() {
        Some("compact") => "compact",
        Some("wide") => "wide",
        _ => "medium",
    };
    let class_name = class_name.unwrap_or_default();
    let focusable = focusable.unwrap_or(true);
    let portal = portal.unwrap_or(false);
    let legacy_dom = legacy_dom.unwrap_or(false);
    let base_trigger_class = if legacy_dom {
        format!("ui-truncated-text ui-truncated-text--{size}")
    } else {
        format!("shared-truncated-text shared-truncated-text--{size}")
    };
    let trigger_class = if class_name.is_empty() {
        base_trigger_class
    } else {
        format!("{base_trigger_class} {class_name}")
    };
    let tooltip_class_name =
        if legacy_dom { "ui-truncated-text__tooltip" } else { "shared-truncated-text__tooltip" };
    let title = (!legacy_dom).then(|| value.clone());
    let data_full_value = (!legacy_dom).then(|| value.clone());
    let truncated = use_signal(|| false);
    let mut focused = use_signal(|| false);
    let hovered = use_signal(|| false);
    let popup_hovered = use_signal(|| false);
    let popup_focused = use_signal(|| false);
    let pinned = use_signal(|| false);
    let dismissed = use_signal(|| false);
    let close_revision = use_signal(|| 0_u64);
    let tooltip_position = use_signal(|| None::<TooltipPosition>);
    let tooltip_id = use_signal(|| {
        format!("shared-truncated-tooltip-{}", NEXT_TOOLTIP_ID.fetch_add(1, Ordering::Relaxed))
    });

    #[cfg(target_arch = "wasm32")]
    let mut mounted_element = use_signal(|| None::<web_sys::Element>);
    #[cfg(target_arch = "wasm32")]
    let mut observer_handle = use_signal(|| None::<ResizeObserverHandle>);
    #[cfg(target_arch = "wasm32")]
    let mut resize_measure_handle = use_signal(|| None::<ResizeMeasureHandle>);
    #[cfg(target_arch = "wasm32")]
    {
        let mut cleanup_measure_handle = resize_measure_handle;
        use_drop(move || cleanup_measure_handle.set(None));
    }
    #[cfg(target_arch = "wasm32")]
    let portal_element = use_signal(|| None::<web_sys::HtmlElement>);
    #[cfg(target_arch = "wasm32")]
    {
        let cleanup_element = portal_element;
        use_drop(move || remove_tooltip(cleanup_element));
    }
    #[cfg(target_arch = "wasm32")]
    let dismiss_handle = use_hook(|| Rc::new(RefCell::new(None::<TooltipDismissHandle>)));
    let mut interaction = TooltipInteraction {
        position: tooltip_position,
        focused,
        hovered,
        popup_hovered,
        popup_focused,
        pinned,
        dismissed,
        close_revision,
        #[cfg(target_arch = "wasm32")]
        portal: portal_element,
    };
    #[cfg(target_arch = "wasm32")]
    {
        // 原生 DOM 回调没有 Dioxus 上下文；稳定回调恢复本组件 scope，让延迟任务随组件卸载取消。
        let defer_close = use_callback(move |()| interaction.defer_close());
        let cleanup_handle = Rc::clone(&dismiss_handle);
        use_drop(move || {
            cleanup_handle.borrow_mut().take();
        });
        let effect_handle = Rc::clone(&dismiss_handle);
        use_effect(move || {
            if tooltip_position().is_none() {
                effect_handle.borrow_mut().take();
                interaction.pinned.set(false);
                return;
            }
            if effect_handle.borrow().is_some() {
                return;
            }
            let Some(window) = web_sys::window() else {
                return;
            };
            let resize_callback = Closure::<dyn FnMut()>::new(move || {
                interaction.close();
            });
            let popup_id = tooltip_id();
            let scroll_popup_id = popup_id.clone();
            let scroll_callback =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                    // 浮层自身的滚动必须保留内容；页面或外层容器滚动后关闭以免定位过期。
                    let target =
                        event.target().and_then(|target| target.dyn_into::<web_sys::Node>().ok());
                    let inside_popup = web_sys::window()
                        .and_then(|window| window.document())
                        .and_then(|document| document.get_element_by_id(&scroll_popup_id))
                        .is_some_and(|popup| {
                            target.as_ref().is_some_and(|target| popup.contains(Some(target)))
                        });
                    if !inside_popup {
                        interaction.close();
                    }
                });
            let resize_added = window
                .add_event_listener_with_callback(
                    "resize",
                    resize_callback.as_ref().unchecked_ref(),
                )
                .is_ok();
            let scroll_added = window
                .add_event_listener_with_callback_and_bool(
                    "scroll",
                    scroll_callback.as_ref().unchecked_ref(),
                    true,
                )
                .is_ok();
            if !resize_added || !scroll_added {
                if resize_added {
                    let _ = window.remove_event_listener_with_callback(
                        "resize",
                        resize_callback.as_ref().unchecked_ref(),
                    );
                }
                if scroll_added {
                    let _ = window.remove_event_listener_with_callback_and_bool(
                        "scroll",
                        scroll_callback.as_ref().unchecked_ref(),
                        true,
                    );
                }
                return;
            }
            let popup_listeners = window
                .document()
                .and_then(|document| document.get_element_by_id(&popup_id))
                .and_then(|element| {
                    PopupListeners::attach(
                        element,
                        format!("{popup_id}-trigger"),
                        interaction,
                        defer_close,
                    )
                });
            *effect_handle.borrow_mut() = Some(TooltipDismissHandle {
                window,
                resize_callback,
                scroll_callback,
                _popup_listeners: popup_listeners,
            });
        });
    }

    let tooltip = tooltip_position();
    let tooltip_id = tooltip_id();
    let described_by = tooltip.map(|_| tooltip_id.clone());
    let tabindex = (focusable && truncated()).then_some("0");
    let data_truncated = truncated().then_some("true");
    #[cfg(target_arch = "wasm32")]
    let request = TooltipRequest {
        value: value.clone(),
        tooltip_id: tooltip_id.clone(),
        class_name: tooltip_class_name.to_owned(),
        portal,
    };
    #[cfg(target_arch = "wasm32")]
    let enter_request = request.clone();
    #[cfg(target_arch = "wasm32")]
    let focus_request = request.clone();
    #[cfg(target_arch = "wasm32")]
    let click_request = request.clone();
    #[cfg(target_arch = "wasm32")]
    let keyboard_request = request;

    rsx! {
        span {
            id: "{tooltip_id}-trigger",
            class: trigger_class,
            title,
            tabindex,
            aria_describedby: described_by,
            role: (focusable && truncated()).then_some("button"),
            aria_expanded: (focusable && truncated()).then(|| tooltip.is_some().to_string()),
            "data-truncated": data_truncated,
            "data-full-value": data_full_value,
            onmounted: move |event| {
                #[cfg(target_arch = "wasm32")]
                {
                    let Some(element) = event.data().downcast::<web_sys::Element>().cloned() else {
                        return;
                    };
                    mounted_element.set(Some(element.clone()));
                    update_truncated_state(
                        &element,
                        display_differs,
                        truncated,
                        tooltip_position,
                        portal_element,
                    );

                    let measure_element = element.clone();
                    let measure_callback = Closure::<dyn FnMut()>::new(move || {
                        update_truncated_state(
                            &measure_element,
                            display_differs,
                            truncated,
                            tooltip_position,
                            portal_element,
                        );
                    });
                    let Some(window) = web_sys::window() else {
                        return;
                    };
                    if window
                        .add_event_listener_with_callback(
                            "resize",
                            measure_callback.as_ref().unchecked_ref(),
                        )
                        .is_ok()
                    {
                        resize_measure_handle.set(Some(ResizeMeasureHandle {
                            window,
                            callback: measure_callback,
                        }));
                    }

                    let observed_element = element.clone();
                    let callback = Closure::<
                        dyn FnMut(js_sys::Array, web_sys::ResizeObserver),
                    >::new(move |_entries, _observer| {
                        update_truncated_state(
                            &observed_element,
                            display_differs,
                            truncated,
                            tooltip_position,
                            portal_element,
                        );
                    });
                    let Ok(observer) = web_sys::ResizeObserver::new(
                        callback.as_ref().unchecked_ref(),
                    ) else {
                        return;
                    };
                    observer.observe(&element);
                    observer_handle.set(Some(ResizeObserverHandle {
                        observer,
                        _callback: callback,
                    }));
                }
                #[cfg(not(target_arch = "wasm32"))]
                let _ = event;
            },
            onmouseenter: move |_| {
                interaction.hovered.set(true);
                interaction.dismissed.set(false);
                #[cfg(target_arch = "wasm32")]
                show_tooltip(
                    mounted_element(),
                    display_differs,
                    truncated,
                    tooltip_position,
                    portal_element,
                    enter_request.clone(),
                );
            },
            onmouseleave: move |_| {
                interaction.hovered.set(false);
                interaction.defer_close();
            },
            onfocus: move |_| {
                focused.set(true);
                #[cfg(target_arch = "wasm32")]
                if !*interaction.dismissed.peek() {
                    show_tooltip(
                        mounted_element(),
                        display_differs,
                        truncated,
                        tooltip_position,
                        portal_element,
                        focus_request.clone(),
                    );
                }
            },
            onblur: move |_| {
                focused.set(false);
                interaction.dismissed.set(false);
                interaction.defer_close();
            },
            onclick: move |event| {
                if focusable && truncated() {
                    event.stop_propagation();
                    #[cfg(target_arch = "wasm32")]
                    toggle_tooltip(interaction, mounted_element(), display_differs, truncated, click_request.clone());
                }
            },
            onkeydown: move |event| {
                if event.key() == Key::Escape && tooltip_position().is_some() {
                    event.stop_propagation();
                    event.prevent_default();
                    interaction.close();
                } else if focusable && truncated() && (event.key() == Key::Enter || event.key() == Key::Character(" ".to_owned())) {
                    event.prevent_default();
                    event.stop_propagation();
                    #[cfg(target_arch = "wasm32")]
                    toggle_tooltip(interaction, mounted_element(), display_differs, truncated, keyboard_request.clone());
                }
            },
            "{display}"
        }
        if let Some(position) = tooltip.filter(|_| !portal) {
            span {
                id: tooltip_id.clone(),
                class: tooltip_class_name,
                role: "tooltip",
                tabindex: "0",
                "data-placement": position.placement,
                style: "--truncated-text-tooltip-left: {position.left}px; --truncated-text-tooltip-top: {position.top}px; --truncated-text-tooltip-width: {position.width}px; --truncated-text-tooltip-max-height: {position.maximum_height}px;",
                "{value}"
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn toggle_tooltip(
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

#[cfg(target_arch = "wasm32")]
fn update_truncated_state(
    element: &web_sys::Element,
    display_differs: bool,
    mut truncated: Signal<bool>,
    tooltip_position: Signal<Option<TooltipPosition>>,
    portal_element: Signal<Option<web_sys::HtmlElement>>,
) {
    let next = element_is_truncated(element, display_differs);
    truncated.set(next);
    if !next {
        hide_tooltip(tooltip_position, portal_element);
    }
}

#[cfg(target_arch = "wasm32")]
fn element_is_truncated(element: &web_sys::Element, display_differs: bool) -> bool {
    display_differs || element.scroll_width() > element.client_width()
}

#[cfg(target_arch = "wasm32")]
fn show_tooltip(
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
    truncated.set(next);
    if !next {
        hide_tooltip(tooltip_position, portal_element);
        return;
    }

    let rectangle = element.get_bounding_client_rect();
    let (viewport_width, viewport_height) = web_sys::window()
        .map(|window| {
            (
                window.inner_width().ok().and_then(|value| value.as_f64()).unwrap_or(1024.0),
                window.inner_height().ok().and_then(|value| value.as_f64()).unwrap_or(768.0),
            )
        })
        .unwrap_or((1024.0, 768.0));
    let width = 360.0_f64.min((viewport_width - 16.0).max(1.0));
    let gap = 8.0;
    let margin = 8.0;
    let available_above = (rectangle.top() - gap - margin).max(1.0);
    let available_below = (viewport_height - rectangle.bottom() - gap - margin).max(1.0);
    let above = available_above > available_below;
    let maximum_left = (viewport_width - width - margin).max(margin);
    let position = TooltipPosition {
        left: rectangle.left().max(margin).min(maximum_left),
        top: if above { rectangle.top() - gap } else { rectangle.bottom() + gap },
        width,
        maximum_height: if above { available_above } else { available_below },
        placement: if above { "above" } else { "below" },
    };
    if request.portal
        && !render_portal_tooltip(
            &mut portal_element,
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

#[cfg(target_arch = "wasm32")]
fn hide_tooltip(
    mut tooltip_position: Signal<Option<TooltipPosition>>,
    mut portal_element: Signal<Option<web_sys::HtmlElement>>,
) {
    tooltip_position.set(None);
    if let Some(element) = portal_element()
        && let Some(parent) = element.parent_node()
    {
        let _ = parent.remove_child(&element);
    }
    portal_element.set(None);
}

#[cfg(target_arch = "wasm32")]
fn remove_tooltip(mut portal_element: Signal<Option<web_sys::HtmlElement>>) {
    if let Some(element) = portal_element()
        && let Some(parent) = element.parent_node()
    {
        let _ = parent.remove_child(&element);
    }
    portal_element.set(None);
}

#[cfg(target_arch = "wasm32")]
fn render_portal_tooltip(
    portal_element: &mut Signal<Option<web_sys::HtmlElement>>,
    value: &str,
    tooltip_id: &str,
    tooltip_class_name: &str,
    position: TooltipPosition,
) -> bool {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return false;
    };
    let Some(body) = document.body() else {
        return false;
    };
    let tooltip = match portal_element() {
        Some(tooltip) => {
            if tooltip.parent_node().is_none() && body.append_child(&tooltip).is_err() {
                return false;
            }
            tooltip
        }
        None => {
            let Ok(element) = document.create_element("span") else {
                return false;
            };
            let Ok(tooltip) = element.dyn_into::<web_sys::HtmlElement>() else {
                return false;
            };
            if body.append_child(&tooltip).is_err() {
                return false;
            }
            tooltip
        }
    };
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
