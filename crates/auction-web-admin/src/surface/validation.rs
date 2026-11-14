//! Admin 表单的字段级校验辅助。
//! 字段错误跟随控件即时展示：控件带 `aria-invalid`，错误文本经 `aria-describedby` 关联到
//! 控件旁的错误节点；提交失败时焦点移到第一个错误字段。这里只处理输入层校验，
//! 服务端业务错误仍走页面级反馈，但可明确归属字段的 400 类错误尽量落到对应字段。

use auction_web_shared::a11y::focus_element_by_id_after_render;
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
/// 错误节点在事件回调返回后才写进 DOM，因此等渲染完成再聚焦。
pub(super) fn focus_field(field_id: &'static str) {
    focus_element_by_id_after_render(field_id);
}
