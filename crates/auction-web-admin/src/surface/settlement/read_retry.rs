//! 结算页各项 GET 的重试控件与成功时的焦点交接。

#[cfg(target_arch = "wasm32")]
use auction_web_shared::a11y::{focus_element_by_id, top_modal};
use auction_web_shared::{AsyncState, AsyncStateKind, t};
use dioxus::prelude::*;

/// 可分别重试的结算读取。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SettlementRead {
    Catalog,
    CatalogLookup,
    Settlement,
    Recovery,
}

impl SettlementRead {
    const fn retry_id(self) -> &'static str {
        match self {
            Self::Catalog => "settlement-catalog-retry",
            Self::CatalogLookup => "settlement-period-retry",
            Self::Settlement => "settlement-report-retry",
            Self::Recovery => "settlement-recovery-retry",
        }
    }

    const fn heading_id(self) -> &'static str {
        match self {
            Self::Catalog | Self::CatalogLookup => "settlement-selector-title",
            Self::Settlement => "settlement-report-panel-title",
            Self::Recovery => "settlement-recovery-title",
        }
    }
}

/// 重试按钮仍持有焦点时，在发布成功结果、拆除按钮之前还焦。
pub(super) fn focus_after_successful_read(_read: SettlementRead, _scope: &str) {
    #[cfg(target_arch = "wasm32")]
    if let Some(document) = web_sys::window().and_then(|window| window.document())
        && let Some(active) = document.active_element()
        && active.id() == _read.retry_id()
        && active.get_attribute("data-read-scope").as_deref() == Some(_scope)
        && top_modal(&document).is_none()
    {
        focus_element_by_id(_read.heading_id());
    }
}

/// 请求期间保留可聚焦的按钮，阻止键盘与指针激活。
#[component]
pub(super) fn SettlementReadRetry(
    read: SettlementRead,
    scope: String,
    message: String,
    pending: bool,
    on_retry: EventHandler<()>,
) -> Element {
    rsx! {
        for scope in [scope] {
            div {
                key: "{scope}",
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: message.clone(),
                    detail: "",
                }
                button {
                    id: read.retry_id(),
                    "data-read-scope": "{scope}",
                    r#type: "button",
                    class: "ui-async-state__retry",
                    aria_describedby: read.heading_id(),
                    aria_disabled: pending.then_some("true"),
                    onclick: move |_| {
                        if !pending {
                            on_retry.call(());
                        }
                    },
                    {t!("admin-retry")}
                }
            }
        }
    }
}
