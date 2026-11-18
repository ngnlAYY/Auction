//! 长文本浮层的悬停宽限、触控固定与 DOM 事件清理；不持有业务内容。

#[cfg(any(target_arch = "wasm32", test))]
use std::cell::RefCell;

use dioxus::prelude::*;

use super::TooltipPosition;
#[cfg(any(target_arch = "wasm32", test))]
use super::document::{self, Document, PortalElement};
#[cfg(any(target_arch = "wasm32", test))]
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
    #[cfg(any(target_arch = "wasm32", test))]
    pub(super) portal: Signal<Option<PortalElement>>,
}

impl TooltipInteraction {
    #[cfg(any(target_arch = "wasm32", test))]
    fn dismiss_for_escape(self, popup_has_focus: bool, restore_focus: impl FnOnce()) -> bool {
        if self.position.peek().is_none() {
            return false;
        }
        self.close();
        // 悬停可以在任意其它控件获焦时打开提示；只有即将消失的浮层持有焦点才还焦。
        if popup_has_focus {
            restore_focus();
        }
        true
    }

    pub(super) fn close(mut self) {
        self.position.set(None);
        self.pinned.set(false);
        self.popup_hovered.set(false);
        self.popup_focused.set(false);
        self.dismissed.set(true);
        #[cfg(any(target_arch = "wasm32", test))]
        super::tooltip::remove_tooltip(self.portal);
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
const POPUP_EVENTS: &[&str] = &["mouseenter", "mouseleave", "focusin", "focusout"];

#[cfg(any(target_arch = "wasm32", test))]
thread_local! {
    static OPEN_TOOLTIPS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

#[cfg(any(target_arch = "wasm32", test))]
pub(super) fn use_tooltip_escape(tooltip_id: Signal<String>, state: TooltipInteraction) {
    use_document_escape(
        move || (state.position)().is_some(),
        move || attach_tooltip_escape(document::current()?, tooltip_id.peek().clone(), state),
    );
}

#[cfg(any(target_arch = "wasm32", test))]
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

#[cfg(any(target_arch = "wasm32", test))]
fn escape_tooltip(document: &Document, tooltip_id: &str, state: TooltipInteraction) -> bool {
    let dialog = document::top_dialog(document);
    let newest = OPEN_TOOLTIPS.with(|open| {
        open.borrow().iter().rev().find_map(|id| {
            let popup = document.get_element_by_id(id)?;
            if dialog.as_ref().is_none_or(|dialog| document::contains(dialog, &popup)) {
                Some((id.clone(), popup))
            } else {
                None
            }
        })
    });
    let Some((_, popup)) = newest.filter(|(id, _)| id == tooltip_id) else { return false };
    let popup_has_focus =
        document.active_element().is_some_and(|focused| document::contains(&popup, &focused));
    state.dismiss_for_escape(popup_has_focus, || {
        if let Some(trigger) = document.get_element_by_id(&format!("{tooltip_id}-trigger"))
            && dialog.as_ref().is_none_or(|dialog| document::contains(dialog, &trigger))
        {
            document::focus_trigger(document, &trigger);
        }
    })
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
