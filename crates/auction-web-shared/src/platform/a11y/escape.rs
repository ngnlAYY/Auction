//! 展开控件的文档级 Escape 监听及清理。

use dioxus::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

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
