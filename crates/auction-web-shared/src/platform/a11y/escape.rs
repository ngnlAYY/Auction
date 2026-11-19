//! 展开控件的文档级 Escape 监听及清理。

use dioxus::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

#[cfg(target_arch = "wasm32")]
use super::ARIA_MODAL_SELECTOR;

/// 注销一次文档监听，并释放回调及其捕获的状态。
pub type EscapeUnsubscribe = Box<dyn FnOnce()>;

/// 只在控件展开时登记监听。DOM 绑定由调用方提供，生命周期与关闭清理共用。
pub fn use_document_escape(
    is_open: impl Fn() -> bool + 'static,
    attach: impl Fn() -> Option<EscapeUnsubscribe> + 'static,
) {
    let mut listener = use_signal(|| None::<EscapeUnsubscribe>);
    use_drop(move || detach(listener));
    use_effect(move || {
        if is_open() {
            if listener.peek().is_none() {
                listener.set(attach());
            }
        } else {
            detach(listener);
        }
    });
}

fn detach(mut listener: Signal<Option<EscapeUnsubscribe>>) {
    let unsubscribe = listener.write().take();
    if let Some(unsubscribe) = unsubscribe {
        unsubscribe();
    }
}

pub(crate) fn with_native_modal_selector<T, E>(
    query: impl Fn(&str) -> Result<T, E>,
) -> Result<T, E> {
    // 空匹配表示支持 :modal 且没有原生模态；只在选择器不可用时回退到打开的 dialog。
    query("dialog:modal").or_else(|_error| query("dialog[open]"))
}

pub(crate) fn top_modal_with<T, E>(
    native_modal: impl Fn(&str) -> Result<Option<T>, E>,
    last_application_dialog: impl FnOnce(Option<&T>) -> Option<T>,
) -> Option<T> {
    let native = with_native_modal_selector(native_modal).ok().flatten();
    last_application_dialog(native.as_ref()).or(native)
}

/// 确定 Escape 的前景模态范围。原生顶层有优先权，其内部仍可打开应用模态。
#[cfg(target_arch = "wasm32")]
pub fn top_modal(document: &web_sys::Document) -> Option<web_sys::Element> {
    top_modal_with(
        |selector| -> Result<_, wasm_bindgen::JsValue> {
            Ok(last_element(document.query_selector_all(selector)?).map(|last| {
                // 原生对话框的打开顺序可能不同于 DOM 顺序，先取持有焦点的原生模态。
                document
                    .active_element()
                    .and_then(|focus| focus.closest(selector).ok().flatten())
                    .unwrap_or(last)
            }))
        },
        |native| {
            let dialogs = match native {
                Some(native) => native.query_selector_all(ARIA_MODAL_SELECTOR),
                None => document.query_selector_all(ARIA_MODAL_SELECTOR),
            }
            .ok()?;
            last_element(dialogs)
        },
    )
}

#[cfg(target_arch = "wasm32")]
fn last_element(nodes: web_sys::NodeList) -> Option<web_sys::Element> {
    nodes.item(nodes.length().checked_sub(1)?)?.dyn_into().ok()
}

/// 绑定一个原生键盘监听。只有回调接手的 Escape 会停止传播和默认行为。
#[cfg(target_arch = "wasm32")]
pub fn bind_document_escape(
    document: web_sys::Document,
    capture: bool,
    on_escape: impl Fn() -> bool + 'static,
) -> Option<EscapeUnsubscribe> {
    let callback =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |event: web_sys::KeyboardEvent| {
            if event.key() == "Escape"
                && !event.is_composing()
                && !event.default_prevented()
                && on_escape()
            {
                event.prevent_default();
                event.stop_propagation();
            }
        });
    document
        .add_event_listener_with_callback_and_bool(
            "keydown",
            callback.as_ref().unchecked_ref(),
            capture,
        )
        .ok()?;
    Some(Box::new(move || {
        let _ = document.remove_event_listener_with_callback_and_bool(
            "keydown",
            callback.as_ref().unchecked_ref(),
            capture,
        );
    }))
}
