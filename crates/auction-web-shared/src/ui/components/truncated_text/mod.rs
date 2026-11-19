//! 长文本截断展示与可操作浮层；悬停、聚焦和点击共享同一份完整文本。

mod document;
#[cfg(not(target_arch = "wasm32"))]
mod host_document;
mod interaction;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
mod tooltip;

#[cfg(not(target_arch = "wasm32"))]
pub use host_document::TooltipBrowserFixture;

use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(target_arch = "wasm32")]
use std::{cell::RefCell, rc::Rc};

use dioxus::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

#[cfg(target_arch = "wasm32")]
use super::resize;
use document::PortalElement;
#[cfg(target_arch = "wasm32")]
use interaction::PopupListeners;
use interaction::TooltipInteraction;
use interaction::use_tooltip_escape;
use tooltip::remove_tooltip;
use tooltip::update_truncated_state;

static NEXT_TOOLTIP_ID: AtomicUsize = AtomicUsize::new(1);

#[derive(Clone, Copy)]
struct AnchorOrder(Memo<usize>);

/// 为按记录标识复用的表行提供当前顺序，供这一行的截断提示判断锚点是否移动。
pub fn use_truncated_text_anchor_order(order: usize) {
    let order = use_memo(use_reactive((&order,), |(order,)| order));
    use_context_provider(|| AnchorOrder(order));
}

#[derive(Clone, Copy, PartialEq)]
struct TooltipPosition {
    left: f64,
    top: f64,
    width: f64,
    maximum_height: f64,
    placement: &'static str,
}

struct TooltipRequest {
    trigger_id: String,
    tooltip_id: String,
    portal: bool,
    value: String,
    #[cfg(target_arch = "wasm32")]
    class_name: String,
}

#[cfg(target_arch = "wasm32")]
struct TooltipDismissHandle {
    window: web_sys::Window,
    viewport: Option<web_sys::VisualViewport>,
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
        if let Some(viewport) = &self.viewport {
            let _ = viewport.remove_event_listener_with_callback(
                "resize",
                self.resize_callback.as_ref().unchecked_ref(),
            );
            let _ = viewport.remove_event_listener_with_callback(
                "scroll",
                self.scroll_callback.as_ref().unchecked_ref(),
            );
        }
    }
}

/// 展示可能很长的标识或 metadata，并在确实发生截断时提供可聚焦 tooltip。
/// `display` 用于调用方已提供缩写文本的场景，展示值与完整值不同也视为截断。浏览器端用
/// `ResizeObserver` 重测宽度，响应式变化后仍只把真正需要提示的文本加入 Tab 顺序；当
/// `focusable=false` 时内联浮层也不加入 Tab 顺序，避免在按钮或链接中产生嵌套焦点。
/// legacy DOM 模式省略 `title` 降级，只输出旧版显式 tooltip。
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
    let display_differs = display.as_ref().is_some_and(|display| display != &value);
    let display_differs =
        use_memo(use_reactive((&display_differs,), |(display_differs,)| display_differs));
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
    let mut measure_subscription = use_signal(|| None::<resize::MeasureSubscription>);
    #[cfg(target_arch = "wasm32")]
    {
        let mut cleanup_subscription = measure_subscription;
        use_drop(move || cleanup_subscription.set(None));
    }
    let portal_element = use_signal(|| None::<PortalElement>);
    use_drop(move || remove_tooltip(portal_element));
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
        portal: portal_element,
    };
    use_tooltip_escape(tooltip_id, interaction);
    let anchor_order = try_consume_context::<AnchorOrder>();
    let mut revealed_order = use_signal(|| None::<usize>);
    use_effect(move || {
        // 同一记录的行可能只移位，宽度和内容保持原样。收起绑定在旧位置的浮层。
        if tooltip_position().is_some()
            && let Some(AnchorOrder(order)) = anchor_order
            && revealed_order.peek().is_some_and(|revealed| revealed != order())
        {
            interaction.close_programmatically(&tooltip_id.peek());
        }
    });
    // 固定宽度文本变化不触发 ResizeObserver；内容提交到 DOM 后重测，且不保留旧 portal 正文。
    use_effect(use_reactive((&value, &display), move |_| {
        #[cfg(target_arch = "wasm32")]
        let element = mounted_element.peek().clone();
        #[cfg(not(target_arch = "wasm32"))]
        let element = document::current().and_then(|document| {
            document.get_element_by_id(&format!("{}-trigger", tooltip_id.peek()))
        });
        if let Some(element) = element {
            interaction.close_programmatically(&tooltip_id.peek());
            update_truncated_state(
                &element,
                *display_differs.peek(),
                truncated,
                interaction,
                &tooltip_id.peek(),
            );
        }
    }));
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
                interaction.close_programmatically(&tooltip_id.peek());
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
                        interaction.close_programmatically(&scroll_popup_id);
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
            // 手机软键盘与页面缩放只改变可见视口时，window 不一定派发 resize/scroll。
            // 沿用窗口变动时关闭浮层的策略，避免保留已经偏离触发文本的旧坐标。
            let viewport = window.visual_viewport();
            let mut handle = TooltipDismissHandle {
                window,
                viewport,
                resize_callback,
                scroll_callback,
                _popup_listeners: None,
            };
            if let Some(viewport) = &handle.viewport {
                let resize_added = viewport
                    .add_event_listener_with_callback(
                        "resize",
                        handle.resize_callback.as_ref().unchecked_ref(),
                    )
                    .is_ok();
                let scroll_added = viewport
                    .add_event_listener_with_callback(
                        "scroll",
                        handle.scroll_callback.as_ref().unchecked_ref(),
                    )
                    .is_ok();
                if !resize_added || !scroll_added {
                    // handle 同时清理已注册的 window 与可见视口监听，包含部分注册失败路径。
                    return;
                }
            }
            handle._popup_listeners = handle
                .window
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
            *effect_handle.borrow_mut() = Some(handle);
        });
    }

    let tooltip = tooltip_position();
    let tooltip_id_signal = tooltip_id;
    let tooltip_id = tooltip_id_signal();
    let described_by = tooltip.map(|_| tooltip_id.clone());
    let tabindex = (focusable && truncated()).then_some("0");
    let data_truncated = truncated().then_some("true");
    let reveal = {
        let request = TooltipRequest {
            trigger_id: format!("{tooltip_id}-trigger"),
            tooltip_id: tooltip_id.clone(),
            portal,
            value: value.clone(),
            #[cfg(target_arch = "wasm32")]
            class_name: tooltip_class_name.to_owned(),
        };
        use_callback(move |pin: bool| {
            if pin && *interaction.pinned.peek() {
                interaction.close();
                return;
            }
            if pin {
                interaction.dismissed.set(false);
            }
            tooltip::show_tooltip(*display_differs.peek(), truncated, interaction, &request);
            if let Some(AnchorOrder(order)) = anchor_order {
                revealed_order.set(Some(order()));
            }
            if pin {
                interaction.pinned.set(interaction.position.peek().is_some());
            }
        })
    };

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
                        *display_differs.peek(),
                        truncated,
                        interaction,
                        &tooltip_id_signal.peek(),
                    );

                    // 元素尺寸与窗口尺寸变化共用全页唯一的监听，只登记本实例的重测回调。
                    let measure_element = element.clone();
                    measure_subscription.set(resize::observe(&element, move || {
                        update_truncated_state(
                            &measure_element,
                            *display_differs.peek(),
                            truncated,
                            interaction,
                            &tooltip_id_signal.peek(),
                        );
                    }));
                }
                #[cfg(not(target_arch = "wasm32"))]
                let _ = event;
            },
            onmouseenter: move |_| {
                interaction.hovered.set(true);
                interaction.dismissed.set(false);
                reveal.call(false);
            },
            onmouseleave: move |_| {
                interaction.hovered.set(false);
                interaction.defer_close();
            },
            onfocus: move |_| {
                focused.set(true);
                if !*interaction.dismissed.peek() {
                    reveal.call(false);
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
                    reveal.call(true);
                }
            },
            onkeydown: move |event| {
                if focusable && truncated() && (event.key() == Key::Enter || event.key() == Key::Character(" ".to_owned())) {
                    event.prevent_default();
                    event.stop_propagation();
                    reveal.call(true);
                }
            },
            "{display}"
        }
        if let Some(position) = tooltip.filter(|_| !portal) {
            span {
                id: tooltip_id.clone(),
                class: tooltip_class_name,
                role: "tooltip",
                tabindex: focusable.then_some("0"),
                "data-placement": position.placement,
                style: "--truncated-text-tooltip-left: {position.left}px; --truncated-text-tooltip-top: {position.top}px; --truncated-text-tooltip-width: {position.width}px; --truncated-text-tooltip-max-height: {position.maximum_height}px;",
                "{value}"
            }
        }
    }
}
