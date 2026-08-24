//! 浏览器端通用无障碍交互。
//!
//! 这里不包含 Public 或 Admin 的业务语义，只处理所有 `aria-modal` 弹窗都需要的键盘焦点、
//! 关闭后恢复和背景滚动边界。事件监听在 surface 根壳层挂载一次，并始终把最内层弹窗视为
//! 当前焦点范围。

#[cfg(target_arch = "wasm32")]
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[cfg(target_arch = "wasm32")]
use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

/// 管理当前 surface 的模态焦点循环、触发控件恢复和背景滚动锁定。
pub fn use_dialog_focus_trap() {
    #[cfg(target_arch = "wasm32")]
    use_effect(move || {
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };

        let opener = Rc::new(RefCell::new(current_focusable(&document)));
        let dialog_open = Rc::new(Cell::new(false));
        let previous_overflow = Rc::new(RefCell::new(None::<String>));

        let focus_document = document.clone();
        let focus_opener = Rc::clone(&opener);
        let focus_callback =
            Closure::<dyn FnMut(web_sys::FocusEvent)>::new(move |event: web_sys::FocusEvent| {
                if top_dialog(&focus_document).is_some() {
                    return;
                }
                let Some(target) = event.target() else {
                    return;
                };
                let Ok(element) = target.dyn_into::<web_sys::HtmlElement>() else {
                    return;
                };
                if is_focusable(&element) {
                    focus_opener.replace(Some(element));
                }
            });
        let _ = document
            .add_event_listener_with_callback("focusin", focus_callback.as_ref().unchecked_ref());
        focus_callback.forget();

        let callback_document = document.clone();
        let keyboard_callback = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |event: web_sys::KeyboardEvent| {
                if event.key() != "Tab" || event.default_prevented() {
                    return;
                }
                let Some(dialog) = top_dialog(&callback_document) else {
                    return;
                };
                let Ok(nodes) = dialog.query_selector_all(FOCUSABLE_SELECTOR) else {
                    return;
                };
                let mut controls = Vec::new();
                for index in 0..nodes.length() {
                    let Some(node) = nodes.item(index) else {
                        continue;
                    };
                    let Ok(control) = node.dyn_into::<web_sys::HtmlElement>() else {
                        continue;
                    };
                    if !is_focusable(&control) {
                        continue;
                    }
                    controls.push(control);
                }
                let (Some(first), Some(last)) = (controls.first(), controls.last()) else {
                    return;
                };
                let active = callback_document.active_element();
                let inside =
                    active.as_ref().is_some_and(|element| dialog.contains(Some(element.as_ref())));
                let at_first = active
                    .as_ref()
                    .is_some_and(|element| element.is_same_node(Some(first.as_ref())));
                let at_last = active
                    .as_ref()
                    .is_some_and(|element| element.is_same_node(Some(last.as_ref())));
                if inside && !(event.shift_key() && at_first || !event.shift_key() && at_last) {
                    return;
                }
                event.prevent_default();
                let target = if event.shift_key() { last } else { first };
                let _ = target.focus();
            },
        );
        let _ = document.add_event_listener_with_callback(
            "keydown",
            keyboard_callback.as_ref().unchecked_ref(),
        );
        keyboard_callback.forget();

        synchronize_dialog_state(&document, &dialog_open, &opener, &previous_overflow);
        let observer_document = document.clone();
        let observer_open = Rc::clone(&dialog_open);
        let observer_opener = Rc::clone(&opener);
        let observer_overflow = Rc::clone(&previous_overflow);
        let observer_callback = Closure::<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>::new(
            move |_records: js_sys::Array, _observer: web_sys::MutationObserver| {
                synchronize_dialog_state(
                    &observer_document,
                    &observer_open,
                    &observer_opener,
                    &observer_overflow,
                );
            },
        );
        let Ok(observer) =
            web_sys::MutationObserver::new(observer_callback.as_ref().unchecked_ref())
        else {
            observer_callback.forget();
            return;
        };
        let options = web_sys::MutationObserverInit::new();
        options.set_child_list(true);
        options.set_subtree(true);
        if let Some(root) = document.document_element() {
            let _ = observer.observe_with_options(root.as_ref(), &options);
        }
        observer_callback.forget();
        std::mem::forget(observer);
    });
}

#[cfg(target_arch = "wasm32")]
fn top_dialog(document: &web_sys::Document) -> Option<web_sys::Element> {
    let dialogs = document.query_selector_all("[role=\"dialog\"][aria-modal=\"true\"]").ok()?;
    dialogs.item(dialogs.length().checked_sub(1)?)?.dyn_into::<web_sys::Element>().ok()
}

#[cfg(target_arch = "wasm32")]
fn current_focusable(document: &web_sys::Document) -> Option<web_sys::HtmlElement> {
    let element = document.active_element()?.dyn_into::<web_sys::HtmlElement>().ok()?;
    is_focusable(&element).then_some(element)
}

#[cfg(target_arch = "wasm32")]
fn is_focusable(element: &web_sys::HtmlElement) -> bool {
    element.matches(FOCUSABLE_SELECTOR).unwrap_or(false)
        && !element.hidden()
        && element.get_attribute("aria-hidden").as_deref() != Some("true")
}

#[cfg(target_arch = "wasm32")]
fn synchronize_dialog_state(
    document: &web_sys::Document,
    dialog_open: &Cell<bool>,
    opener: &RefCell<Option<web_sys::HtmlElement>>,
    previous_overflow: &RefCell<Option<String>>,
) {
    let next_open = top_dialog(document).is_some();
    if next_open == dialog_open.get() {
        return;
    }
    dialog_open.set(next_open);

    let Some(body) = document.body() else {
        return;
    };
    if next_open {
        previous_overflow
            .replace(Some(body.style().get_property_value("overflow").unwrap_or_default()));
        let _ = body.style().set_property("overflow", "hidden");
        return;
    }

    if let Some(previous) = previous_overflow.take() {
        if previous.is_empty() {
            let _ = body.style().remove_property("overflow");
        } else {
            let _ = body.style().set_property("overflow", &previous);
        }
    }
    if let Some(target) = opener.borrow().clone().filter(|target| target.is_connected()) {
        let _ = target.focus();
    }
}

#[cfg(target_arch = "wasm32")]
const FOCUSABLE_SELECTOR: &str = "a[href],button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex=\"-1\"]),[contenteditable=\"true\"]";
