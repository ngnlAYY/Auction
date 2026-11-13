//! 长文本浮层的悬停宽限、触控固定与 DOM 事件清理；不持有业务内容。

use dioxus::prelude::*;

use super::TooltipPosition;

#[derive(Clone, Copy)]
pub(super) struct TooltipInteraction {
    pub(super) position: Signal<Option<TooltipPosition>>,
    pub(super) focused: Signal<bool>,
    pub(super) hovered: Signal<bool>,
    pub(super) popup_hovered: Signal<bool>,
    pub(super) popup_focused: Signal<bool>,
    pub(super) pinned: Signal<bool>,
    pub(super) dismissed: Signal<bool>,
    pub(super) close_revision: Signal<u64>,
    #[cfg(target_arch = "wasm32")]
    pub(super) portal: Signal<Option<web_sys::HtmlElement>>,
}

impl TooltipInteraction {
    pub(super) fn close(mut self) {
        self.position.set(None);
        self.pinned.set(false);
        self.popup_hovered.set(false);
        self.popup_focused.set(false);
        self.dismissed.set(true);
        #[cfg(target_arch = "wasm32")]
        super::hide_tooltip(self.position, self.portal);
    }

    /// 给指针跨越触发文本与浮层之间的间隙留出时间；旧任务不能关闭新一轮浮层。
    pub(super) fn defer_close(mut self) {
        let revision = self.close_revision.peek().saturating_add(1);
        self.close_revision.set(revision);
        spawn(async move {
            crate::timer::TimeoutFuture::new(180).await;
            if *self.close_revision.peek() == revision
                && !*self.focused.peek()
                && !*self.hovered.peek()
                && !*self.popup_hovered.peek()
                && !*self.popup_focused.peek()
                && !*self.pinned.peek()
            {
                self.close();
            }
        });
    }
}

#[cfg(target_arch = "wasm32")]
const POPUP_EVENTS: &[&str] = &["mouseenter", "mouseleave", "focusin", "focusout", "keydown"];

#[cfg(target_arch = "wasm32")]
pub(super) struct PopupListeners {
    element: web_sys::Element,
    document: web_sys::Document,
    callback: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>,
    outside: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>,
}

#[cfg(target_arch = "wasm32")]
impl PopupListeners {
    /// 同时支持 Dioxus 内联浮层和挂到 body 的 portal；关闭时成对清理监听器。
    /// 延迟关闭经由组件创建的回调恢复 Runtime 与 Scope，不能在原生 DOM 回调中直接 spawn。
    pub(super) fn attach(
        element: web_sys::Element,
        trigger_id: String,
        mut state: TooltipInteraction,
        defer_close: EventHandler<()>,
    ) -> Option<Self> {
        use wasm_bindgen::{JsCast, closure::Closure};
        let document = web_sys::window()?.document()?;
        let focus_document = document.clone();
        let focus_trigger = trigger_id.clone();
        let callback = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            match event.type_().as_str() {
                "mouseenter" => state.popup_hovered.set(true),
                "mouseleave" => {
                    state.popup_hovered.set(false);
                    defer_close.call(());
                }
                "focusin" => state.popup_focused.set(true),
                "focusout" => {
                    state.popup_focused.set(false);
                    defer_close.call(());
                }
                "keydown"
                    if event
                        .dyn_ref::<web_sys::KeyboardEvent>()
                        .is_some_and(|event| event.key() == "Escape") =>
                {
                    event.prevent_default();
                    event.stop_propagation();
                    state.close();
                    if let Some(trigger) = focus_document
                        .get_element_by_id(&focus_trigger)
                        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
                    {
                        let _ = trigger.focus();
                    }
                }
                _ => {}
            }
        });
        let outside_document = document.clone();
        let outside_element = element.clone();
        let outside = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            let Some(target) =
                event.target().and_then(|target| target.dyn_into::<web_sys::Node>().ok())
            else {
                return;
            };
            let inside_trigger = outside_document
                .get_element_by_id(&trigger_id)
                .is_some_and(|trigger| trigger.contains(Some(&target)));
            if !inside_trigger && !outside_element.contains(Some(&target)) {
                state.close();
            }
        });
        let listeners = Self { element, document, callback, outside };
        for event in POPUP_EVENTS {
            listeners
                .element
                .add_event_listener_with_callback(
                    event,
                    listeners.callback.as_ref().unchecked_ref(),
                )
                .ok()?;
        }
        listeners
            .document
            .add_event_listener_with_callback(
                "pointerdown",
                listeners.outside.as_ref().unchecked_ref(),
            )
            .ok()?;
        Some(listeners)
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for PopupListeners {
    fn drop(&mut self) {
        use wasm_bindgen::JsCast;
        for event in POPUP_EVENTS {
            let _ = self
                .element
                .remove_event_listener_with_callback(event, self.callback.as_ref().unchecked_ref());
        }
        let _ = self.document.remove_event_listener_with_callback(
            "pointerdown",
            self.outside.as_ref().unchecked_ref(),
        );
    }
}
