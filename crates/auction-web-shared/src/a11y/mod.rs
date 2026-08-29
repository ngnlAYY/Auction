//! 浏览器端通用无障碍交互。
//!
//! 这里不包含 Public 或 Admin 的业务语义，只处理所有 `aria-modal` 弹窗都需要的键盘焦点、
//! 关闭后恢复和背景滚动边界。事件监听在 surface 根壳层挂载一次，并始终把最内层弹窗视为
//! 当前焦点范围。

#[cfg(target_arch = "wasm32")]
use std::{cell::RefCell, rc::Rc};

#[cfg(target_arch = "wasm32")]
use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

/// 管理当前 surface 的模态焦点循环、触发控件恢复和可选的背景滚动锁定。
pub fn use_dialog_focus_trap(_lock_background_scroll: bool) {
    #[cfg(target_arch = "wasm32")]
    {
        let guard = use_hook(|| Rc::new(RefCell::new(None::<DialogFocusTrapGuard>)));
        let cleanup_guard = Rc::clone(&guard);
        use_drop(move || {
            if let Some(guard) = cleanup_guard.borrow_mut().take() {
                guard.cleanup();
            }
        });

        use_effect(move || {
            let Some(document) = web_sys::window().and_then(|window| window.document()) else {
                return;
            };

            let opener = Rc::new(RefCell::new(current_focusable(&document)));
            let active_dialog = Rc::new(RefCell::new(None::<web_sys::Element>));
            let previous_overflow = Rc::new(RefCell::new(None::<String>));

            let focus_document = document.clone();
            let focus_opener = Rc::clone(&opener);
            let focus_callback = Closure::<dyn FnMut(web_sys::FocusEvent)>::new(
                move |event: web_sys::FocusEvent| {
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
                },
            );
            let _ = document.add_event_listener_with_callback(
                "focusin",
                focus_callback.as_ref().unchecked_ref(),
            );

            let click_document = document.clone();
            let click_opener = Rc::clone(&opener);
            let click_callback = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(
                move |event: web_sys::MouseEvent| {
                    if top_dialog(&click_document).is_some() {
                        return;
                    }
                    let Some(target) = event.target() else {
                        return;
                    };
                    let Ok(element) = target.dyn_into::<web_sys::Element>() else {
                        return;
                    };
                    let Some(control) = element
                        .closest(FOCUSABLE_SELECTOR)
                        .ok()
                        .flatten()
                        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
                        .filter(is_focusable)
                    else {
                        return;
                    };
                    // 捕获阶段先于 Dioxus 打开 dialog，程序化 click 也能像旧页面的 currentTarget ref
                    // 一样保存真实触发控件；dialog 内关闭按钮不会覆盖该 opener。
                    click_opener.replace(Some(control));
                },
            );
            let _ = document.add_event_listener_with_callback_and_bool(
                "click",
                click_callback.as_ref().unchecked_ref(),
                true,
            );

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
                    let inside = active
                        .as_ref()
                        .is_some_and(|element| dialog.contains(Some(element.as_ref())));
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

            synchronize_dialog_state(
                &document,
                &active_dialog,
                &opener,
                &previous_overflow,
                _lock_background_scroll,
            );
            let observer_document = document.clone();
            let observer_dialog = Rc::clone(&active_dialog);
            let observer_opener = Rc::clone(&opener);
            let observer_overflow = Rc::clone(&previous_overflow);
            let observer_callback =
                Closure::<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>::new(
                    move |_records: js_sys::Array, _observer: web_sys::MutationObserver| {
                        synchronize_dialog_state(
                            &observer_document,
                            &observer_dialog,
                            &observer_opener,
                            &observer_overflow,
                            _lock_background_scroll,
                        );
                    },
                );
            let observer =
                web_sys::MutationObserver::new(observer_callback.as_ref().unchecked_ref()).ok();
            if let Some(observer) = observer.as_ref() {
                let options = web_sys::MutationObserverInit::new();
                options.set_child_list(true);
                options.set_subtree(true);
                if let Some(root) = document.document_element() {
                    let _ = observer.observe_with_options(root.as_ref(), &options);
                }
            }
            guard.replace(Some(DialogFocusTrapGuard {
                document,
                focus_callback,
                click_callback,
                keyboard_callback,
                _observer_callback: observer_callback,
                observer,
                opener,
                previous_overflow,
            }));
        });
    }
}

#[cfg(target_arch = "wasm32")]
struct DialogFocusTrapGuard {
    document: web_sys::Document,
    focus_callback: Closure<dyn FnMut(web_sys::FocusEvent)>,
    click_callback: Closure<dyn FnMut(web_sys::MouseEvent)>,
    keyboard_callback: Closure<dyn FnMut(web_sys::KeyboardEvent)>,
    _observer_callback: Closure<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>,
    observer: Option<web_sys::MutationObserver>,
    opener: Rc<RefCell<Option<web_sys::HtmlElement>>>,
    previous_overflow: Rc<RefCell<Option<String>>>,
}

#[cfg(target_arch = "wasm32")]
impl DialogFocusTrapGuard {
    /// surface 卸载时移除全局监听并恢复滚动与触发控件，避免重新登录后叠加第二套 trap。
    fn cleanup(self) {
        if let Some(observer) = self.observer.as_ref() {
            observer.disconnect();
        }
        let _ = self.document.remove_event_listener_with_callback(
            "focusin",
            self.focus_callback.as_ref().unchecked_ref(),
        );
        let _ = self.document.remove_event_listener_with_callback_and_bool(
            "click",
            self.click_callback.as_ref().unchecked_ref(),
            true,
        );
        let _ = self.document.remove_event_listener_with_callback(
            "keydown",
            self.keyboard_callback.as_ref().unchecked_ref(),
        );
        if let Some(body) = self.document.body()
            && let Some(previous) = self.previous_overflow.borrow_mut().take()
        {
            if previous.is_empty() {
                let _ = body.style().remove_property("overflow");
            } else {
                let _ = body.style().set_property("overflow", &previous);
            }
        }
        if let Some(target) = self.opener.borrow().clone().filter(|target| target.is_connected()) {
            let _ = target.focus();
        }
    }
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
    active_dialog: &RefCell<Option<web_sys::Element>>,
    opener: &RefCell<Option<web_sys::HtmlElement>>,
    previous_overflow: &RefCell<Option<String>>,
    lock_background_scroll: bool,
) {
    let next_dialog = top_dialog(document);
    let unchanged = match (active_dialog.borrow().as_ref(), next_dialog.as_ref()) {
        (Some(current), Some(next)) => current.is_same_node(Some(next.as_ref())),
        (None, None) => true,
        _ => false,
    };
    if unchanged {
        return;
    }
    // 不能只记录“是否打开”：创建成功后会在同一次 DOM 更新中把表单弹窗替换成密钥弹窗。
    // 此时继续保留最初 opener，但必须把焦点移入新的最顶层 dialog。
    let had_dialog = active_dialog.borrow().is_some();
    active_dialog.replace(next_dialog.clone());

    let Some(body) = document.body() else {
        return;
    };
    if let Some(dialog) = next_dialog {
        let active_inside = current_focusable(document)
            .as_ref()
            .is_some_and(|element| dialog.contains(Some(element.as_ref())));
        if !active_inside {
            // opener 已由 focusin 或捕获阶段 click 记录。这里不能再用当前焦点覆盖它：程序化
            // click 不会先聚焦按钮，DOM 切换瞬间的 activeElement 仍可能是上一个控件。
            if let Some(target) = preferred_dialog_focus(&dialog) {
                let _ = target.focus();
            }
        }
        if !had_dialog && lock_background_scroll {
            previous_overflow
                .replace(Some(body.style().get_property_value("overflow").unwrap_or_default()));
            let _ = body.style().set_property("overflow", "hidden");
        }
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
fn preferred_dialog_focus(dialog: &web_sys::Element) -> Option<web_sys::HtmlElement> {
    if let Some(target) = dialog
        .query_selector("[autofocus]")
        .ok()
        .flatten()
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
        .filter(is_focusable)
    {
        return Some(target);
    }

    let nodes = dialog.query_selector_all(FOCUSABLE_SELECTOR).ok()?;
    for index in 0..nodes.length() {
        let Some(node) = nodes.item(index) else {
            continue;
        };
        let Ok(control) = node.dyn_into::<web_sys::HtmlElement>() else {
            continue;
        };
        if is_focusable(&control) {
            return Some(control);
        }
    }
    None
}

#[cfg(target_arch = "wasm32")]
const FOCUSABLE_SELECTOR: &str = "a[href],button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex=\"-1\"]),[contenteditable=\"true\"]";
