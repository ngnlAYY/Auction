//! 长文本浮层的悬停宽限、触控固定与 DOM 事件清理；不持有业务内容。

use std::cell::RefCell;

use dioxus::prelude::*;
#[cfg(target_arch = "wasm32")]
use web_sys::HtmlElement as PortalElement;

use super::TooltipPosition;
use super::document::{self, Document};
#[cfg(not(target_arch = "wasm32"))]
use super::host_document::DomPortal as PortalElement;
#[cfg(any(target_arch = "wasm32", test))]
use super::tracking::AnchorFrames;
use crate::a11y::{EscapeUnsubscribe, use_document_escape};

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
    pub(super) portal: Signal<Option<PortalElement>>,
    #[cfg(any(target_arch = "wasm32", test))]
    pub(super) anchor_frames: CopyValue<Option<AnchorFrames>>,
}

impl TooltipInteraction {
    pub(super) fn close_programmatically(self, tooltip_id: &str) {
        if self.position.peek().is_some() {
            self.dismiss(document::current().as_ref(), tooltip_id, None);
        }
    }

    pub(super) fn dismiss(
        self,
        document: Option<&Document>,
        tooltip_id: &str,
        previous_parent: Option<&document::Element>,
    ) -> bool {
        if self.position.peek().is_none() {
            return false;
        }
        // 必须在卸载浮层前确定焦点归属。悬停打开的提示不会抢走其它控件的焦点。
        let restore = document.and_then(|document| {
            let popup = document.get_element_by_id(tooltip_id)?;
            let focus = document.active_element()?;
            if !document::contains(&popup, &focus) {
                return None;
            }
            let trigger = document
                .get_element_by_id(&format!("{tooltip_id}-trigger"))
                .or_else(|| previous_parent.cloned())?;
            Some((document, trigger))
        });
        self.close();
        if let Some((document, trigger)) = restore {
            document::focus_trigger(document, &trigger);
        }
        true
    }

    pub(super) fn close(mut self) {
        #[cfg(any(target_arch = "wasm32", test))]
        self.stop_anchor_tracking();
        self.position.set(None);
        self.pinned.set(false);
        self.popup_hovered.set(false);
        self.popup_focused.set(false);
        self.dismissed.set(true);
        super::tooltip::remove_tooltip(self.portal);
    }

    #[cfg(any(target_arch = "wasm32", test))]
    pub(super) fn stop_anchor_tracking(mut self) {
        drop(self.anchor_frames.write().take());
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
                // 没有打开的浮层时 close 只会留下 dismissed，吞掉下一次聚焦的揭示。
                && self.position.peek().is_some()
            {
                self.close();
            }
        });
    }
}

#[cfg(target_arch = "wasm32")]
const POPUP_EVENTS: &[&str] = &["mouseenter", "mouseleave", "focusin", "focusout"];

thread_local! {
    static OPEN_TOOLTIPS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn use_tooltip_escape(tooltip_id: Signal<String>, state: TooltipInteraction) {
    use_document_escape(
        move || (state.position)().is_some(),
        move || attach_tooltip_escape(document::current()?, tooltip_id.peek().clone(), state),
    );
}

fn attach_tooltip_escape(
    document: Document,
    tooltip_id: String,
    state: TooltipInteraction,
) -> Option<EscapeUnsubscribe> {
    let event_document = document.clone();
    let event_tooltip_id = tooltip_id.clone();
    // 捕获阶段先处理当前模态中的提示，遮罩后面的提示把按键留给前景抽屉或对话框。
    let unsubscribe = document::listen_escape(document, move || {
        escape_tooltip(&event_document, &event_tooltip_id, state)
    })?;
    OPEN_TOOLTIPS.with(|open| open.borrow_mut().push(tooltip_id.clone()));
    Some(Box::new(move || {
        unsubscribe();
        OPEN_TOOLTIPS.with(|open| open.borrow_mut().retain(|id| id != &tooltip_id));
    }))
}

fn escape_tooltip(document: &Document, tooltip_id: &str, state: TooltipInteraction) -> bool {
    let dialog = document::top_dialog(document);
    let focused = document.active_element();
    if dialog.as_ref().is_some_and(|dialog| {
        focused.as_ref().is_none_or(|focused| !document::contains(dialog, focused))
    }) {
        return false;
    }
    let owns_escape = OPEN_TOOLTIPS.with(|open| {
        open.borrow()
            .iter()
            .rev()
            .find_map(|id| {
                let popup = document.get_element_by_id(id)?;
                let trigger = document.get_element_by_id(&format!("{id}-trigger"))?;
                dialog
                    .as_ref()
                    .is_none_or(|dialog| {
                        document::contains(dialog, &popup) && document::contains(dialog, &trigger)
                    })
                    .then_some(id)
            })
            .is_some_and(|id| id == tooltip_id)
    });
    owns_escape && state.dismiss(Some(document), tooltip_id, None)
}

#[cfg(target_arch = "wasm32")]
pub(super) struct PopupListeners {
    element: web_sys::Element,
    document: web_sys::Document,
    callback: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>,
    outside: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>,
}

#[cfg(target_arch = "wasm32")]
impl PopupListeners {
    /// 同时支持内联和挂到模态或正文的浮层；关闭时成对清理监听器。
    /// 延迟关闭经由组件创建的回调恢复 Runtime 与 Scope，不能在原生 DOM 回调中直接 spawn。
    pub(super) fn attach(
        element: web_sys::Element,
        trigger_id: String,
        mut state: TooltipInteraction,
        defer_close: EventHandler<()>,
    ) -> Option<Self> {
        use wasm_bindgen::{JsCast, closure::Closure};
        let document = web_sys::window()?.document()?;
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
