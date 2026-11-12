//! Public 静态服务说明页。
//! 旧版 `/status` 只说明 Public surface 的用途；健康检查和实时事件仍由机器 API 与页头连接
//! 状态承担，不在这里重复暴露诊断表或建立第二条订阅。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use super::LiveConnectionState;

/// 根据 PublicShell 的实时连接状态读取当前语言的辅助文案。
pub(crate) fn live_connection_message(state: LiveConnectionState) -> String {
    match state {
        LiveConnectionState::Connecting => t!("realtime-connection-connecting"),
        LiveConnectionState::Degraded => t!("realtime-connection-degraded"),
        LiveConnectionState::Live => t!("realtime-connection-live"),
        LiveConnectionState::Manual => t!("realtime-connection-manual"),
    }
}

pub(super) fn page() -> Element {
    rsx! {
        section { class: "public-status-page", aria_labelledby: "public-status-title",
            h1 { id: "public-status-title", {t!("page-status-title")} }
            p { {t!("page-status-note")} }
        }
    }
}
