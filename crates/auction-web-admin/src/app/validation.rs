//! Admin 表单的字段级校验辅助。
//!
//! 字段错误跟随控件即时展示：控件带 `aria-invalid`，错误文本经 `aria-describedby` 关联到
//! 控件旁的错误节点；提交失败时焦点移到第一个错误字段。这里只处理输入层校验，
//! 服务端业务错误仍走页面级反馈，但可明确归属字段的 400 类错误尽量落到对应字段。

use dioxus::prelude::*;

/// 渲染字段旁的错误提示；无错误时不渲染任何节点，`aria-describedby` 也不应再指向该 id。
#[component]
pub(super) fn FieldError(
    /// 错误节点 id；必须与控件 `aria-describedby` 使用的值一致。
    id: String,
    /// 已翻译的错误文案；None 表示该字段当前无错误。
    message: Option<String>,
) -> Element {
    match message {
        // role="alert" 让错误出现即被辅助技术播报，不必等用户移动焦点。
        Some(message) => rsx! {
            p { class: "admin-field-error", id, role: "alert", {message} }
        },
        None => rsx! {},
    }
}

/// 提交失败后把焦点移到指定字段（调用方传入表单序上第一个错误字段的控件 id）。
///
/// Dioxus 在事件回调返回后才会把错误节点写进 DOM，因此让出一个宏任务再取元素。
pub(super) fn focus_field(field_id: &'static str) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;

        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(0).await;
            let target = web_sys::window()
                .and_then(|window| window.document())
                .and_then(|document| document.get_element_by_id(field_id))
                .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok());
            if let Some(target) = target {
                let _ = target.focus();
            }
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    let _ = field_id;
}
