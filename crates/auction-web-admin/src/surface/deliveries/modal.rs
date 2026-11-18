//! 投递确认使用浏览器原生模态能力，旧浏览器沿用现有弹窗外壳。

use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;

/// 由浏览器处理顶层、惰性背景和焦点；取消始终是初始焦点。
#[component]
pub(crate) fn DeliveryModal(
    dialog_id: &'static str,
    title: String,
    busy: bool,
    on_cancel: EventHandler<()>,
    children: Element,
) -> Element {
    #[cfg(target_arch = "wasm32")]
    {
        let mut native = use_signal(|| true);
        if native() {
            let title_id = format!("{dialog_id}-title");
            return rsx! {
                style {
                    "dialog.admin-delivery-modal:modal {{ overscroll-behavior: contain; }}
                     dialog.admin-delivery-modal::backdrop {{ overflow: hidden; overscroll-behavior: contain; background: var(--color-overlay); }}"
                }
                dialog {
                    id: dialog_id,
                    class: "ui-dialog__panel admin-delivery-modal",
                    aria_labelledby: title_id.clone(),
                    onmounted: move |event| {
                        if !event.data().downcast::<web_sys::Element>().is_some_and(|element| invoke_dialog(element, "showModal")) {
                            native.set(false);
                        }
                    },
                    oncancel: move |event| {
                        if busy {
                            event.prevent_default();
                        } else {
                            on_cancel.call(());
                        }
                    },
                    div { class: "ui-dialog__header",
                        h2 { id: title_id, class: "ui-dialog__title", "{title}" }
                        button {
                            r#type: "button",
                            class: "ui-dialog__close",
                            disabled: busy,
                            autofocus: true,
                            onclick: move |_| {
                                close_delivery_modal(dialog_id);
                                on_cancel.call(());
                            },
                            {t!("delivery-cancel")}
                        }
                    }
                    {children}
                }
            };
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = dialog_id;
    rsx! { Dialog { title, close_label: t!("delivery-cancel"), busy, on_close: on_cancel, {children} } }
}

/// 明确关闭让浏览器把焦点交还打开弹窗的控件。
pub(crate) fn close_delivery_modal(dialog_id: &str) {
    #[cfg(target_arch = "wasm32")]
    if let Some(element) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(dialog_id))
    {
        invoke_dialog(&element, "close");
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = dialog_id;
}

#[cfg(target_arch = "wasm32")]
fn invoke_dialog(element: &web_sys::Element, method: &str) -> bool {
    use wasm_bindgen::JsCast as _;

    js_sys::Reflect::get(element.as_ref(), &method.into())
        .ok()
        .and_then(|function| function.dyn_into::<js_sys::Function>().ok())
        .is_some_and(|function| function.call0(element.as_ref()).is_ok())
}
