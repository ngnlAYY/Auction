//! 浏览器端通用无障碍交互。
//! 不含 surface 业务语义，只处理所有 `aria-modal` 弹窗都需要的键盘焦点、关闭后恢复和
//! 背景滚动边界。监写在 surface 根壳层挂载一次，始终把最内层弹窗视为当前焦点范围。

#[cfg(target_arch = "wasm32")]
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[cfg(target_arch = "wasm32")]
use dioxus::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

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
            let Some(document) = web_sys::window().and_then(|window| window.document()) else {
                return;
            };
            let Some(target) = document
                .get_element_by_id(_target_id)
                .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
            else {
                return;
            };
            let _ = target.focus();
        }));
    }
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

            let opener = Rc::new(RefCell::new(current_focusable(&document)));
            let active_dialog = Rc::new(RefCell::new(None::<web_sys::Element>));
            let previous_overflow = Rc::new(RefCell::new(None::<String>));

            let focus_document = document.clone();
            let focus_opener = Rc::clone(&opener);
            let focus_callback = Closure::<dyn FnMut(web_sys::FocusEvent)>::new(
                move |event: web_sys::FocusEvent| {
                    if let Some(dialog) = top_dialog(&focus_document) {
                        let inside = event
                            .target()
                            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                            .is_some_and(|target| dialog.contains(Some(target.as_ref())));
                        if !inside && let Some(target) = preferred_dialog_focus(&dialog) {
                            let _ = target.focus();
                        }
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
        // focus 会同步派发焦点事件；先释放借用，再把控制权交给浏览器监听器。
        let opener = self.opener.borrow().clone();
        if let Some(target) = opener.filter(|target| target.is_connected()) {
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
        && !element.matches(":disabled").unwrap_or(false)
        && element.is_connected()
        // 手机折叠菜单、关闭的 details 与隐藏祖先不能成为焦点循环的首尾控件。
        && element.closest("[hidden], [inert], [aria-hidden=\"true\"]").ok().flatten().is_none()
        && element.get_client_rects().length() > 0
        && web_sys::window().and_then(|window| window.get_computed_style(element).ok().flatten())
            .is_none_or(|style| !matches!(style.get_property_value("visibility").ok().as_deref(), Some("hidden" | "collapse")))
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
        if let Some(dialog) = next_dialog
            && !document.active_element().is_some_and(|element| {
                dialog.contains(Some(element.as_ref()))
                    && (element.is_same_node(Some(dialog.as_ref()))
                        || element.dyn_ref::<web_sys::HtmlElement>().is_some_and(is_focusable))
            })
            && let Some(target) = preferred_dialog_focus(&dialog)
        {
            let _ = target.focus();
        }
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
    // focus 会同步触发 focusin，监听器会写回 opener；快照必须先释放 RefCell 只读借用，
    // 避免 if let 条件的临时借用跨越 focus 调用并在关闭弹窗时触发重入 panic。
    let opener = opener.borrow().clone();
    if let Some(target) = opener.filter(|target| target.is_connected()) {
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
    dialog.clone().dyn_into::<web_sys::HtmlElement>().ok()
}

#[cfg(target_arch = "wasm32")]
const FOCUSABLE_SELECTOR: &str = "a[href],button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex=\"-1\"]),[contenteditable=\"true\"]";
