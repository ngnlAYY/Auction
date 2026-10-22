//! 统一模态弹窗外壳。
//! 只负责 markup 与关闭语义：焦点循环、初始聚焦、关闭后焦点恢复与背景滚动锁由外壳层的
//! [`use_dialog_focus_trap`](crate::use_dialog_focus_trap) 全局处理（它自动识别
//! `[role="dialog"][aria-modal="true"]`）。
//! 遮罩点击始终不关闭弹窗，避免误触丢失草稿；正常关闭用 Escape、关闭或内容区取消按钮。
//! `busy=true` 时 Escape 与关闭按钮禁用，内容区取消按钮由调用方同步保护。
//! 移动形态不靠额外 props：`base.css` 中 `.ui-dialog` 在 ≤48rem（bp-sm）下自动切为贴底抽屉。
//! 文案一律由调用方经 props 传入，本 crate 不持有任何 i18n key。

use std::sync::atomic::{AtomicUsize, Ordering};

use dioxus::html::Key;
use dioxus::prelude::*;

/// 同一页面可能同时存在多个弹窗（嵌套或队列），`aria-labelledby` 需要全页唯一的 id。
static NEXT_DIALOG_ID: AtomicUsize = AtomicUsize::new(1);

/// 模态弹窗外壳：遮罩、居中/贴底面板、标题栏与受 busy 守卫的 Escape/关闭按钮。
/// `children` 是内容槽；`on_close` 只在 `busy=false` 时经任一关闭路径触发。焦点陷阱始终
/// 把 DOM 中最内层弹窗视为当前焦点范围。
#[component]
pub fn Dialog(
    /// 弹窗标题；渲染为标题栏 `h2` 并通过 `aria-labelledby` 接线到面板。
    title: String,
    /// 关闭按钮文案（同时作为其 `aria-label`）。
    close_label: String,
    /// 繁忙（通常为提交中）：为 true 时 Escape / 关闭按钮禁用；遮罩点击始终不关闭。
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
            // 背景点击只阻止冒泡，既不丢失表单，也不触发外层页面或嵌套弹窗的点击动作。
            onclick: move |event| event.stop_propagation(),
            // Escape 关闭挂在遮罩根上，弹窗内按键会冒泡到这里；stop_propagation 避免嵌套
            // 弹窗时一次 Escape 连关两层。
            onkeydown: move |event| {
                if event.key() == Key::Escape {
                    event.prevent_default();
                    event.stop_propagation();
                    if !busy {
                        on_close.call(());
                    }
                }
            },
            section {
                class: "ui-dialog__panel",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "{title_id}",
                // 提交时可能没有可用子控件，面板仍须承接初始焦点与 Tab 循环。
                tabindex: "-1",
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
