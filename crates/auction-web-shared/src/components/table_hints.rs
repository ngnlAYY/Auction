//! 普通表格单元格与表头的完整文本提示；复杂文本继续由 TruncatedText 拥有。

/// 为动态插入的表格提供委托式悬浮提示；不修改单元格内容或浏览器交互的默认行为。
pub(super) fn use_table_hints() {
    #[cfg(target_arch = "wasm32")]
    {
        use dioxus::prelude::*;
        use std::{cell::RefCell, rc::Rc};
        use wasm_bindgen::{JsCast, closure::Closure};

        let guard = use_hook(|| Rc::new(RefCell::new(None::<TableHintsGuard>)));
        let cleanup = Rc::clone(&guard);
        use_drop(move || {
            cleanup.borrow_mut().take();
        });
        use_effect(move || {
            let Some(document) = web_sys::window().and_then(|window| window.document()) else {
                return;
            };
            let Some(body) = document.body() else { return };
            let active = Rc::new(RefCell::new(None::<web_sys::Element>));
            let event_active = Rc::clone(&active);
            let callback = Closure::<dyn FnMut(web_sys::Event)>::new(
                move |event: web_sys::Event| {
                    let Some(target) = event
                        .target()
                        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                    else {
                        return;
                    };
                    let cell = target.closest("th, td").ok().flatten();
                    let Some(cell) = cell else {
                        event_active.borrow_mut().take();
                        return;
                    };
                    // 避免与已有完整文本浮层竞争，也不把表单输入或一次性凭据复制进 title。
                    if target.closest(".shared-truncated-text, .ui-truncated-text, .admin-users-history-trigger, [role=dialog]").ok().flatten().is_some()
                    || cell.query_selector("input, textarea, [role=dialog]").ok().flatten().is_some()
                {
                    clear_owned_hint(&cell);
                    if let Some(previous) = event_active.borrow_mut().take() { clear_owned_hint(&previous); }
                    return;
                }
                    let control = target
                        .closest("button, a, summary")
                        .ok()
                        .flatten()
                        .filter(|control| cell.contains(Some(control)));
                    let target = control.unwrap_or(cell);
                    refresh_hint(&target);
                    event_active.replace(Some(target));
                },
            );
            let observer_active = Rc::clone(&active);
            let observer_callback =
                Closure::<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>::new(
                    move |records: js_sys::Array, _observer: web_sys::MutationObserver| {
                        let active = observer_active.borrow().clone();
                        let Some(active) = active else { return };
                        if !active.is_connected() {
                            observer_active.borrow_mut().take();
                            return;
                        }
                        // 无关节点（例如其他行的秒级倒计时）变化时不读取当前单元格的 innerText。
                        if records.iter().any(|record| {
                            record
                                .dyn_into::<web_sys::MutationRecord>()
                                .ok()
                                .and_then(|record| record.target())
                                .is_none_or(|target| active.contains(Some(&target)))
                        }) {
                            refresh_hint(&active);
                        }
                    },
                );
            let Ok(observer) =
                web_sys::MutationObserver::new(observer_callback.as_ref().unchecked_ref())
            else {
                return;
            };
            let options = web_sys::MutationObserverInit::new();
            options.set_child_list(true);
            options.set_character_data(true);
            options.set_subtree(true);
            // 不观察属性：更新 title 不会触发自身，实时快照文字变化才刷新当前提示。
            if observer.observe_with_options(body.as_ref(), &options).is_err() {
                return;
            }
            let listeners = TableHintsGuard {
                document,
                callback,
                observer,
                _observer_callback: observer_callback,
            };
            for event in ["mouseover", "focusin"] {
                if listeners
                    .document
                    .add_event_listener_with_callback(
                        event,
                        listeners.callback.as_ref().unchecked_ref(),
                    )
                    .is_err()
                {
                    return;
                }
            }
            guard.replace(Some(listeners));
        });
    }
}

#[cfg(target_arch = "wasm32")]
fn refresh_hint(element: &web_sys::Element) {
    use wasm_bindgen::JsCast;
    if element.closest("[role=dialog]").ok().flatten().is_some()
        || element.query_selector("input, textarea, [role=dialog]").ok().flatten().is_some()
    {
        clear_owned_hint(element);
        return;
    }
    if element.has_attribute("title") && !element.has_attribute("data-table-hint") {
        return;
    }
    let Some(element_html) = element.dyn_ref::<web_sys::HtmlElement>() else { return };
    let text = element_html.inner_text();
    let text = text.trim();
    if text.is_empty() {
        clear_owned_hint(element);
        return;
    }
    if element.get_attribute("title").as_deref() != Some(text) {
        let _ = element.set_attribute("title", text);
        let _ = element.set_attribute("data-table-hint", "true");
    }
}

#[cfg(target_arch = "wasm32")]
fn clear_owned_hint(element: &web_sys::Element) {
    if element.has_attribute("data-table-hint") {
        let _ = element.remove_attribute("title");
        let _ = element.remove_attribute("data-table-hint");
    }
}

#[cfg(target_arch = "wasm32")]
struct TableHintsGuard {
    document: web_sys::Document,
    callback: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>,
    observer: web_sys::MutationObserver,
    _observer_callback:
        wasm_bindgen::closure::Closure<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for TableHintsGuard {
    fn drop(&mut self) {
        use wasm_bindgen::JsCast;
        self.observer.disconnect();
        for event in ["mouseover", "focusin"] {
            let _ = self
                .document
                .remove_event_listener_with_callback(event, self.callback.as_ref().unchecked_ref());
        }
        if let Ok(elements) = self.document.query_selector_all("[data-table-hint]") {
            for index in 0..elements.length() {
                if let Some(element) =
                    elements.item(index).and_then(|node| node.dyn_into::<web_sys::Element>().ok())
                {
                    clear_owned_hint(&element);
                }
            }
        }
    }
}
