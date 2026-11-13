//! Public 静态服务说明页。
//! 旧版 `/status` 只说明 Public surface 的用途；健康检查和实时事件仍由机器 API 与页头连接
//! 状态承担，不在这里重复暴露诊断表或建立第二条订阅。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use super::{LiveConnectionState, PublicConnectionState};

/// 根据 PublicShell 的实时连接状态读取当前语言的辅助文案。
pub(crate) fn live_connection_message(state: LiveConnectionState) -> String {
    match state {
        LiveConnectionState::Connecting => t!("realtime-connection-connecting"),
        LiveConnectionState::Degraded => t!("realtime-connection-degraded"),
        LiveConnectionState::Live => t!("realtime-connection-live"),
        LiveConnectionState::Manual => t!("realtime-connection-manual"),
    }
}

/// 顶栏徽章从页头六态取文案；实时四态与详情页隐藏状态共用同一组译文。
pub(crate) fn public_connection_message(state: PublicConnectionState) -> String {
    match state {
        PublicConnectionState::Connecting => t!("realtime-connection-connecting"),
        PublicConnectionState::Degraded => t!("realtime-connection-degraded"),
        PublicConnectionState::Live => t!("realtime-connection-live"),
        PublicConnectionState::Manual => t!("realtime-connection-manual"),
        PublicConnectionState::Idle => t!("realtime-connection-idle"),
        PublicConnectionState::Unavailable => t!("realtime-connection-unavailable"),
    }
}

/// 徽章的稳定状态名；同时作为 `data-state` 供样式与诊断使用。
pub(crate) fn public_connection_state_name(state: PublicConnectionState) -> &'static str {
    match state {
        PublicConnectionState::Connecting => "connecting",
        PublicConnectionState::Degraded => "degraded",
        PublicConnectionState::Live => "live",
        PublicConnectionState::Manual => "manual",
        PublicConnectionState::Idle => "idle",
        PublicConnectionState::Unavailable => "unavailable",
    }
}

/// 徽章状态点的修饰 class；中性状态沿用基础灰点，不新增无样式类。
pub(crate) fn public_connection_class(state: PublicConnectionState) -> &'static str {
    match state {
        PublicConnectionState::Live => "public-connection-badge--live",
        PublicConnectionState::Degraded => "public-connection-badge--degraded",
        PublicConnectionState::Unavailable => "public-connection-badge--unavailable",
        PublicConnectionState::Connecting
        | PublicConnectionState::Manual
        | PublicConnectionState::Idle => "",
    }
}

pub(super) fn page() -> Element {
    rsx! {
        section { class: "public-status-page", aria_labelledby: "public-status-title",
            // 页面名称由导航高亮项表达；h1 只保留语义，作为本区域的可访问名称。
            h1 { id: "public-status-title", class: "public-visually-hidden",
                {t!("page-status-title")}
            }
            p { {t!("page-status-note")} }
        }
    }
}
