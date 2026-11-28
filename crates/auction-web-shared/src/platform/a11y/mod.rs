//! 浏览器端通用无障碍交互。
//! 不含 surface 业务语义，只处理所有 `aria-modal` 弹窗都需要的键盘焦点、关闭后恢复和
//! 背景滚动边界。监听在 surface 根壳层挂载一次，始终把最内层弹窗视为当前焦点范围。
//! 另提供文档级 Escape 与按元素 ID 移动焦点的原语，供菜单收起、校验失败等场景使用。

mod escape;
#[cfg(target_arch = "wasm32")]
mod focus;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use escape::top_modal_with;
pub(crate) use escape::with_native_modal_selector;
pub use escape::{EscapeUnsubscribe, use_document_escape};
#[cfg(target_arch = "wasm32")]
pub use escape::{bind_document_escape, top_modal};

#[cfg(target_arch = "wasm32")]
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[cfg(target_arch = "wasm32")]
use dioxus::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

#[cfg(target_arch = "wasm32")]
use focus::{current_focusable, is_focus_target, is_focusable, preferred_dialog_focus};

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct BodyScrollLockState {
    owners: usize,
    original_overflow: Option<String>,
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static BODY_SCROLL_LOCK: RefCell<BodyScrollLockState> = RefCell::new(BodyScrollLockState::default());
}

#[cfg(target_arch = "wasm32")]
fn acquire_body_scroll_lock(body: &web_sys::HtmlElement) {
    BODY_SCROLL_LOCK.with_borrow_mut(|state| {
        if state.owners == 0 {
            state.original_overflow =
                Some(body.style().get_property_value("overflow").unwrap_or_default());
            let _ = body.style().set_property("overflow", "hidden");
        }
        state.owners = state.owners.saturating_add(1);
    });
}

#[cfg(target_arch = "wasm32")]
fn release_body_scroll_lock(body: &web_sys::HtmlElement) {
    BODY_SCROLL_LOCK.with_borrow_mut(|state| {
        if state.owners == 0 {
            return;
        }
        state.owners -= 1;
        if state.owners == 0
            && let Some(previous) = state.original_overflow.take()
        {
            if previous.is_empty() {
                let _ = body.style().remove_property("overflow");
            } else {
                let _ = body.style().set_property("overflow", &previous);
            }
        }
    });
}

/// SPA 路由真正变化后把键盘焦点移入主内容；首次挂载与同路由数据刷新不抢占用户焦点。
pub fn use_route_focus(_route_token: String, _target_id: &'static str) {
    #[cfg(target_arch = "wasm32")]
    {
        let mounted = use_hook(|| Rc::new(Cell::new(false)));
        let mounted_for_effect = Rc::clone(&mounted);
        use_effect(use_reactive((&_route_token,), move |(_route_token,)| {
            // use_effect 首次挂载必定执行；该次保留浏览器自然的文档起始焦点。
            if !mounted_for_effect.replace(true) {
                return;
            }
            focus_element_by_id(_target_id);
        }));
    }
}

/// 把键盘焦点移到指定 ID 的元素，避免菜单或面板收起后焦点落到 `body`；元素不存在或不是
/// `HtmlElement` 时保持当前焦点。宿主构建没有 DOM，是空操作。
pub fn focus_element_by_id(_element_id: &str) {
    #[cfg(target_arch = "wasm32")]
    if let Some(element) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(_element_id))
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    {
        // 聚焦失败只影响焦点位置，不改变任何状态，无需上报。
        let _ = element.focus();
    }
}

/// 让出一个宏任务后再按 ID 聚焦：事件回调里新渲染的节点（如字段错误）要等回调返回、
/// Dioxus 写入 DOM 后才存在，立即查找会落空。任务经 Dioxus 运行时派生，只能在组件或其事件
/// 回调中调用。
pub fn focus_element_by_id_after_render(_element_id: &'static str) {
    #[cfg(target_arch = "wasm32")]
    spawn(async move {
        crate::timer::TimeoutFuture::new(0).await;
        focus_element_by_id(_element_id);
    });
}

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

            let opener = Rc::new(RefCell::new(None::<web_sys::HtmlElement>));
            let click_candidate = Rc::new(RefCell::new(None::<web_sys::HtmlElement>));
            let active_dialog = Rc::new(RefCell::new(None::<web_sys::Element>));
            let scroll_lock_owned = Rc::new(Cell::new(false));

            let focus_document = document.clone();
            let focus_click_candidate = Rc::clone(&click_candidate);
            let focus_callback = Closure::<dyn FnMut(web_sys::FocusEvent)>::new(
                move |event: web_sys::FocusEvent| {
                    let Some(dialog) = top_dialog(&focus_document) else {
                        let candidate_is_focused = event
                            .target()
                            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                            .is_some_and(|target| {
                                focus_click_candidate.borrow().as_ref().is_some_and(|candidate| {
                                    candidate.is_same_node(Some(target.as_ref()))
                                        || candidate.contains(Some(target.as_ref()))
                                })
                            });
                        if !candidate_is_focused {
                            drop(focus_click_candidate.borrow_mut().take());
                        }
                        return;
                    };
                    let inside = event
                        .target()
                        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                        .is_some_and(|target| dialog.contains(Some(target.as_ref())));
                    if !inside && let Some(target) = preferred_dialog_focus(&dialog) {
                        let _ = target.focus();
                    }
                },
            );
            let _ = document.add_event_listener_with_callback(
                "focusin",
                focus_callback.as_ref().unchecked_ref(),
            );

            let click_document = document.clone();
            let click_event_candidate = Rc::clone(&click_candidate);
            let click_callback = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(
                move |event: web_sys::MouseEvent| {
                    if top_dialog(&click_document).is_some() {
                        return;
                    }
                    let control = event
                        .target()
                        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                        .and_then(|element| element.closest(FOCUSABLE_SELECTOR).ok().flatten())
                        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
                        .filter(is_focusable);
                    if let Some(control) = control {
                        // 捕获阶段先于 Dioxus 打开 dialog；候选只在后续 modal 转换时成为 opener。
                        click_event_candidate.replace(Some(control));
                    } else {
                        drop(click_event_candidate.borrow_mut().take());
                    }
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
                    // 焦点循环只用首尾控件，保持原筛选顺序但不收集全部 DOM 句柄。
                    let mut first = None;
                    let mut last = None;
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
                        if first.is_none() {
                            first = Some(control.clone());
                        }
                        last = Some(control);
                    }
                    let (Some(first), Some(last)) = (first, last) else {
                        // busy 弹窗可能没有任何可 Tab 子控件，焦点必须留在面板而非背景。
                        event.prevent_default();
                        if let Ok(panel) = dialog.dyn_into::<web_sys::HtmlElement>() {
                            let _ = panel.focus();
                        }
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
                    // busy 结束后焦点可能仍在 tabindex=-1 的面板上，反向 Tab 应从末项进入。
                    let at_panel = active
                        .as_ref()
                        .is_some_and(|element| element.is_same_node(Some(dialog.as_ref())));
                    if inside
                        && !at_panel
                        && !(event.shift_key() && at_first || !event.shift_key() && at_last)
                    {
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
                &click_candidate,
                &scroll_lock_owned,
                _lock_background_scroll,
            );
            let observer_document = document.clone();
            let observer_dialog = Rc::clone(&active_dialog);
            let observer_opener = Rc::clone(&opener);
            let observer_click_candidate = Rc::clone(&click_candidate);
            let observer_scroll_lock = Rc::clone(&scroll_lock_owned);
            let observer_lock_background_scroll = _lock_background_scroll;
            let observer_callback =
                Closure::<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>::new(
                    move |_records: js_sys::Array, _observer: web_sys::MutationObserver| {
                        synchronize_dialog_state(
                            &observer_document,
                            &observer_dialog,
                            &observer_opener,
                            &observer_click_candidate,
                            &observer_scroll_lock,
                            observer_lock_background_scroll,
                        );
                    },
                );
            let observer =
                web_sys::MutationObserver::new(observer_callback.as_ref().unchecked_ref()).ok();
            if let Some(observer) = observer.as_ref() {
                let options = web_sys::MutationObserverInit::new();
                options.set_child_list(true);
                options.set_subtree(true);
                // busy 切换会禁用已聚焦控件；即使面板身份未变也须重新收敛焦点。
                options.set_attributes(true);
                options.set_attribute_filter(&js_sys::Array::of3(
                    &"disabled".into(),
                    &"hidden".into(),
                    &"inert".into(),
                ));
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
                click_candidate,
                scroll_lock_owned,
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
    click_candidate: Rc<RefCell<Option<web_sys::HtmlElement>>>,
    scroll_lock_owned: Rc<Cell<bool>>,
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
        if self.scroll_lock_owned.get()
            && let Some(body) = self.document.body()
        {
            release_body_scroll_lock(&body);
            self.scroll_lock_owned.set(false);
        }
        // focus 会同步派发焦点事件；先释放借用，再把控制权交给浏览器监听器。
        let opener = self.opener.borrow_mut().take();
        drop(self.click_candidate.borrow_mut().take());
        if let Some(target) = opener.filter(|target| target.is_connected()) {
            let _ = target.focus();
        }
    }
}

/// 自绘模态的语义选择器。原生对话框的焦点循环由浏览器管理。
pub(crate) const ARIA_MODAL_SELECTOR: &str = "[role=\"dialog\"][aria-modal=\"true\"]";

#[cfg(target_arch = "wasm32")]
fn top_dialog(document: &web_sys::Document) -> Option<web_sys::Element> {
    let dialogs = document.query_selector_all(ARIA_MODAL_SELECTOR).ok()?;
    dialogs.item(dialogs.length().checked_sub(1)?)?.dyn_into::<web_sys::Element>().ok()
}

/// 聚焦仍连接、可见且可用的控件。
#[cfg(target_arch = "wasm32")]
pub(crate) fn focus_control(element: web_sys::Element) {
    if let Some(control) = element.dyn_into::<web_sys::HtmlElement>().ok().filter(is_focusable) {
        let _ = control.focus();
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn transition_dialog_opener<T>(
    body_available: bool,
    had_dialog: bool,
    has_dialog: bool,
    candidate: &mut Option<T>,
    opener: &mut Option<T>,
    candidate_is_valid: impl FnOnce(&T) -> bool,
) -> Option<T> {
    if !body_available {
        return None;
    }
    let candidate = candidate.take();
    if has_dialog {
        if !had_dialog {
            *opener = candidate.filter(candidate_is_valid);
        }
        None
    } else {
        opener.take()
    }
}

#[cfg(target_arch = "wasm32")]
fn synchronize_dialog_state(
    document: &web_sys::Document,
    active_dialog: &RefCell<Option<web_sys::Element>>,
    opener: &RefCell<Option<web_sys::HtmlElement>>,
    click_candidate: &RefCell<Option<web_sys::HtmlElement>>,
    scroll_lock_owned: &Cell<bool>,
    lock_background_scroll: bool,
) {
    let next_dialog = top_dialog(document);
    let unchanged = match (active_dialog.borrow().as_ref(), next_dialog.as_ref()) {
        (Some(current), Some(next)) => current.is_same_node(Some(next.as_ref())),
        (None, None) => true,
        _ => false,
    };
    if unchanged {
        if let Some(dialog) = next_dialog
            && !document.active_element().is_some_and(|element| {
                dialog.contains(Some(element.as_ref()))
                    && (element.is_same_node(Some(dialog.as_ref()))
                        || element.dyn_ref::<web_sys::HtmlElement>().is_some_and(is_focus_target))
            })
            && let Some(target) = preferred_dialog_focus(&dialog)
        {
            let _ = target.focus();
        }
        return;
    }
    let body = document.body();
    let had_dialog = active_dialog.borrow().is_some();
    let opener_to_restore = {
        let mut click_candidate_state = click_candidate.borrow_mut();
        let mut opener_state = opener.borrow_mut();
        transition_dialog_opener(
            body.is_some(),
            had_dialog,
            next_dialog.is_some(),
            &mut click_candidate_state,
            &mut opener_state,
            |candidate| candidate.is_connected(),
        )
    };
    let Some(body) = body else {
        return;
    };
    active_dialog.replace(next_dialog.clone());
    if let Some(dialog) = next_dialog {
        let active_inside = current_focusable(document)
            .as_ref()
            .is_some_and(|element| dialog.contains(Some(element.as_ref())));
        if !active_inside {
            // 未从本次激活打开时不继承更早的 opener，避免关闭后抢回无关焦点。
            if let Some(target) = preferred_dialog_focus(&dialog) {
                let _ = target.focus();
            }
        }
        if lock_background_scroll && !scroll_lock_owned.get() {
            acquire_body_scroll_lock(&body);
            scroll_lock_owned.set(true);
        }
        return;
    }

    if scroll_lock_owned.replace(false) {
        release_body_scroll_lock(&body);
    }
    if let Some(target) = opener_to_restore.filter(|target| target.is_connected()) {
        let _ = target.focus();
        // focusin 同步触发；清除恢复动作留下的候选，避免后续程序化弹窗复用它。
        drop(click_candidate.borrow_mut().take());
    }
}

/// 有资格承接焦点的原生控件和显式可聚焦元素。
pub(crate) const FOCUSABLE_SELECTOR: &str = "a[href],button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex=\"-1\"]),[contenteditable=\"true\"]";
#[cfg(test)]
mod tests {
    use super::transition_dialog_opener;

    #[test]
    fn dialog_opener_waits_for_body_and_is_not_reused_after_close() {
        let mut candidate = Some("trigger");
        let mut opener = None;
        assert_eq!(
            transition_dialog_opener(false, false, true, &mut candidate, &mut opener, |_| true),
            None
        );
        assert_eq!(candidate, Some("trigger"));
        assert_eq!(opener, None);

        assert_eq!(
            transition_dialog_opener(true, false, true, &mut candidate, &mut opener, |_| true),
            None
        );
        assert_eq!(candidate, None);
        assert_eq!(opener, Some("trigger"));

        candidate = Some("nested trigger");
        assert_eq!(
            transition_dialog_opener(true, true, true, &mut candidate, &mut opener, |_| true),
            None
        );
        assert_eq!(candidate, None);
        assert_eq!(opener, Some("trigger"));

        candidate = Some("stale candidate");
        assert_eq!(
            transition_dialog_opener(true, true, false, &mut candidate, &mut opener, |_| true),
            Some("trigger")
        );
        assert_eq!(candidate, None);
        assert_eq!(opener, None);

        assert_eq!(
            transition_dialog_opener(true, false, true, &mut candidate, &mut opener, |_| true),
            None
        );
        assert_eq!(opener, None);

        let mut stale_opener = Some("old trigger");
        candidate = Some("detached trigger");
        assert_eq!(
            transition_dialog_opener(true, false, true, &mut candidate, &mut stale_opener, |_| {
                false
            }),
            None
        );
        assert_eq!(candidate, None);
        assert_eq!(stale_opener, None);
    }
}
