//! 管理端跨页面展示辅助组件。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::format_localized_time;

/// 按旧版管理端约定把 UTC 时间显示为当前界面语言的 Asia/Shanghai 时间。
///
/// 旧版使用浏览器 `Intl.DateTimeFormat`，因此不能在 Rust 侧固定月份语言；WASM 路径继续
/// 使用浏览器的本地化格式，非 WASM 构建只保留静态检查所需的稳定兜底。`datetime` 属性
/// 仍保留原始 RFC3339 值，屏幕文案统一经过这一层。
pub fn format_admin_time(value: &str) -> String {
    format_localized_time(value)
}

/// 将 Admin API 的稳定错误码映射为当前语言的可操作提示。
///
/// 未识别的服务端扩展码只能落入通用兜底；底层 HTTP 正文、数据库错误和浏览器异常不会直接
/// 展示。这样既恢复旧版 `errorKey(error)` 行为，也避免把服务端自然语言当成跨语言契约。
pub fn admin_api_error_message(code: &str) -> String {
    match code {
        "invalid_auction_command" => t!("admin-error-invalid-auction-command"),
        "invalid_auction_intake" => t!("admin-error-invalid-auction-intake"),
        "unsupported_equipment_source" => t!("admin-error-unsupported-equipment-source"),
        "equipment_source_unavailable" => t!("admin-error-equipment-source-unavailable"),
        "auction_intake_unavailable" => t!("admin-error-auction-intake-unavailable"),
        "seller_blacklisted" | "auction_intake_player_blacklisted" => {
            t!("admin-error-auction-intake-player-blacklisted")
        }
        "equipment_conflict" => t!("admin-error-equipment-conflict"),
        "network_error" => t!("admin-error-network"),
        "request_timeout" => t!("admin-error-request-timeout"),
        "invalid_response" => t!("admin-error-invalid-response"),
        "invalid_credentials" => t!("admin-error-invalid-credentials"),
        "unauthenticated" | "http_401" | "csrf_unavailable" => {
            t!("admin-error-unauthenticated")
        }
        "manual_early_settlement_disabled" => {
            t!("admin-error-manual-early-settlement-disabled")
        }
        "csrf_invalid" | "origin_forbidden" | "permission_denied" | "http_403" => {
            t!("admin-error-forbidden")
        }
        "rate_limited" | "http_429" => t!("admin-error-rate-limited"),
        "not_found" | "http_404" => t!("admin-error-not-found"),
        "feature_not_supported" | "http_501" => t!("admin-error-feature-not-supported"),
        "storage_unavailable"
        | "temporarily_unavailable"
        | "internal_error"
        | "http_500"
        | "http_502"
        | "http_503"
        | "http_504" => t!("admin-error-storage-unavailable"),
        "reconciliation_required" | "outcome_unknown" => {
            t!("admin-error-reconciliation-required")
        }
        code if code.ends_with("_conflict")
            || code.starts_with("stale_")
            || matches!(
                code,
                "recovery_not_confirmable" | "recovery_evidence_unresolved" | "invalid_lifecycle"
            ) =>
        {
            t!("admin-error-conflict")
        }
        code if code.starts_with("invalid_") || code == "http_400" => {
            t!("admin-error-invalid-request")
        }
        _ => t!("admin-error-generic"),
    }
}

/// 管理动作统一反馈；成功与稳定错误码不能再压缩为一个布尔值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminActionFeedback {
    Success,
    Error(String),
}

impl AdminActionFeedback {
    pub const fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }

    pub fn message(&self, success: String) -> String {
        match self {
            Self::Success => success,
            Self::Error(code) => admin_api_error_message(code),
        }
    }
}

/// 管理端异步状态面板。
///
/// 页面在加载或等待权威数据时复用该结构；它不是未实现功能的静态占位页。
#[component]
pub fn AdminPanel(title: String, detail: String) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-panel-title",
            h1 { id: "admin-panel-title", "{title}" }
            p { class: "admin-panel__note", "{detail}" }
        }
    }
}
