//! 加载 / 错误 / 空结果三态面板。
//!
//! 统一自 Public 的 `RetryPlaceholder`（错误 + 重试）、`Placeholder`（加载/空结果）与
//! Admin 的 `AdminPanel`：三者都是「标题 + 说明 + 可选重试」的语义面板，差异仅在
//! 无障碍角色与是否提供重试入口。文案一律由调用方通过 props 传入。

use std::sync::atomic::{AtomicUsize, Ordering};

use dioxus::prelude::*;

/// 同一页面可能出现多个状态面板，`aria-labelledby` 需要全页唯一的 id。
static NEXT_ASYNC_STATE_ID: AtomicUsize = AtomicUsize::new(1);

/// 异步状态面板的三种语义。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsyncStateKind {
    /// 正在加载：`role="status"` + `aria-live="polite"`，屏幕阅读器礼貌播报。
    Loading,
    /// 加载失败：`role="alert"`（自带 assertive 播报语义），可附带重试按钮。
    Error,
    /// 成功但无数据：`role="status"` + `aria-live="polite"`。
    Empty,
}

impl AsyncStateKind {
    const fn modifier(self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::Error => "error",
            Self::Empty => "empty",
        }
    }

    const fn role(self) -> &'static str {
        match self {
            Self::Error => "alert",
            Self::Loading | Self::Empty => "status",
        }
    }
}

/// 展示异步数据的加载、错误或空结果状态。
///
/// 组件只呈现调用方已经判定的安全文案，不吞掉 API 错误，也不把错误正文直接展示给
/// 浏览器；真实数据页面仍由各自的 API 读取分支负责。`retry_label` 与 `on_retry`
/// 必须同时提供才会渲染重试按钮，避免只给样式没有行为的假按钮。
#[component]
pub fn AsyncState(
    kind: AsyncStateKind,
    /// 面板标题文案。
    title: String,
    /// 面板说明文案。
    detail: String,
    /// 重试按钮文案；缺省时即使提供 `on_retry` 也不渲染按钮。
    #[props(default)]
    retry_label: Option<String>,
    /// 重试按钮回调；缺省时即使提供 `retry_label` 也不渲染按钮。
    #[props(default)]
    on_retry: Option<EventHandler<()>>,
) -> Element {
    let title_id = use_hook(|| {
        format!("async-state-title-{}", NEXT_ASYNC_STATE_ID.fetch_add(1, Ordering::Relaxed))
    });
    let class = format!("ui-async-state ui-async-state--{}", kind.modifier());
    // role="alert" 已隐含 assertive 的 aria-live，不再重复声明，避免部分阅读器双报。
    let aria_live = if kind == AsyncStateKind::Error { None } else { Some("polite") };
    let retry = retry_label.zip(on_retry);

    rsx! {
        section {
            class: "{class}",
            role: kind.role(),
            aria_live,
            aria_labelledby: "{title_id}",
            h2 { id: "{title_id}", class: "ui-async-state__title", "{title}" }
            p { class: "ui-async-state__detail", "{detail}" }
            if let Some((label, on_retry)) = retry {
                button {
                    class: "ui-async-state__retry",
                    r#type: "button",
                    onclick: move |_| on_retry.call(()),
                    "{label}"
                }
            }
        }
    }
}
