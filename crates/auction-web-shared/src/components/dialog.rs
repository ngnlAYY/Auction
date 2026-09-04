//! 统一模态弹窗外壳。
//!
//! 合并自 Public 的 `.public-auction-dialog`（竞价/代理两处）与 Admin 的
//! `.admin-shell__dialog-backdrop` + `.admin-shell__dialog` 等 12 处弹窗：三端语义一致——
//! 遮罩 + 居中面板 + 标题栏 + 关闭按钮。组件只负责 markup 与关闭语义，键盘焦点循环、
//! 初始聚焦、关闭后触发器焦点恢复与背景滚动锁仍由外壳层的
//! [`use_dialog_focus_trap`](crate::use_dialog_focus_trap) 全局处理（它自动识别
//! `[role="dialog"][aria-modal="true"]`）。两端迁移时统一以 `true` 调用该 hook 锁定背景
//! 滚动（原 public=true / admin=false 不一致，统一为 true，弹窗打开期间背景不应滚动）。
//!
//! 关闭语义（与两端刚修齐的 busy 守卫一致）：`busy=true` 时 Escape、遮罩点击、关闭按钮
//! 三条路径全部禁用，`on_close` 保证不会被触发；弹窗内的提交中状态因此不会丢失成功回调。
//!
//! 移动形态：组件不感知断点；`base.css` 中 `.ui-dialog` 在 ≤48rem（bp-sm）媒体查询下
//! 自动切换为贴底抽屉（上口圆角、最大高度约束），无需额外 props。
//!
//! 文案一律由调用方通过 props 传入，本 crate 不持有任何 i18n key。

use std::sync::atomic::{AtomicUsize, Ordering};

use dioxus::html::Key;
use dioxus::prelude::*;

/// 同一页面可能同时存在多个弹窗（嵌套或队列），`aria-labelledby` 需要全页唯一的 id。
static NEXT_DIALOG_ID: AtomicUsize = AtomicUsize::new(1);

/// 模态弹窗外壳：遮罩、居中/贴底面板、标题栏与三条受 busy 守卫的关闭路径。
///
/// `children` 是内容槽，表单与动作按钮等具体交互由调用方提供；`on_close` 只在
/// `busy=false` 时经任一关闭路径触发。需要嵌套弹窗时直接嵌套渲染即可，焦点陷阱始终
/// 把 DOM 中最内层弹窗视为当前焦点范围。
#[component]
pub fn Dialog(
    /// 弹窗标题；渲染为标题栏 `h2` 并通过 `aria-labelledby` 接线到面板。
    title: String,
    /// 关闭按钮文案（同时作为其 `aria-label`）。
    close_label: String,
    /// 繁忙（通常为提交中）：为 true 时 Escape / 遮罩点击 / 关闭按钮全部禁用。
    #[props(default)]
    busy: bool,
    /// 任一关闭路径触发时调用；`busy=true` 时保证不触发。
    on_close: EventHandler<()>,
    children: Element,
) -> Element {
    let title_id =
        use_hook(|| format!("ui-dialog-title-{}", NEXT_DIALOG_ID.fetch_add(1, Ordering::Relaxed)));

    rsx! {
        div {
            class: "ui-dialog",
            role: "presentation",
            // 遮罩点击关闭：面板上的 stop_propagation 保证内容区点击不会误关。
            onclick: move |_| {
                if !busy {
                    on_close.call(());
                }
            },
            // Escape 关闭挂在遮罩根上，弹窗内任意焦点的按键都会冒泡到这里；
            // stop_propagation 避免嵌套弹窗时一次 Escape 连关两层。
            onkeydown: move |event| {
                if event.key() == Key::Escape && !busy {
                    event.prevent_default();
                    event.stop_propagation();
                    on_close.call(());
                }
            },
            section {
                class: "ui-dialog__panel",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "{title_id}",
                onclick: move |event| event.stop_propagation(),
                div { class: "ui-dialog__header",
                    h2 { class: "ui-dialog__title", id: "{title_id}", "{title}" }
                    button {
                        class: "ui-dialog__close",
                        r#type: "button",
                        disabled: busy,
                        aria_label: close_label.clone(),
                        onclick: move |_| on_close.call(()),
                        "{close_label}"
                    }
                }
                {children}
            }
        }
    }
}
