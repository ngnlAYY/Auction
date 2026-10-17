//! 管理端跨页面展示辅助组件。

use auction_web_shared::{
    BaseErrorKind, base_api_error_message, format_localized_time, legacy_t as t,
};

/// 按旧版管理端约定把 UTC 时间显示为当前界面语言的 Asia/Shanghai 时间。
/// 旧版使用浏览器 `Intl.DateTimeFormat`，因此不能在 Rust 侧固定月份语言；WASM 路径继续
/// 使用浏览器的本地化格式，非 WASM 构建只保留静态检查所需的稳定兜底。`datetime` 属性
/// 仍保留原始 RFC3339 值，屏幕文案统一经过这一层。
pub fn format_admin_time(value: &str) -> String {
    format_localized_time(value)
}

/// 将 Admin API 的稳定错误码映射为当前语言的可操作提示。
/// 未识别的服务端扩展码只能落入通用兜底；底层 HTTP 正文、数据库错误和浏览器异常不会直接
/// 展示。这样既恢复旧版 `errorKey(error)` 行为，也避免把服务端自然语言当成跨语言契约。
/// 分类顺序：先匹配 Admin 业务码（含必须留在业务分支的 `recovery_not_confirmable` /
/// `invalid_lifecycle` 等冲突语义码，以及被基础层归入 Unauthenticated 的
/// `invalid_credentials`），未命中再走共享基础层 [`base_api_error_message`] 的通用码 /
/// `http_*` 兑底，最后落到通用兜底译文。
pub fn admin_api_error_message(code: &str) -> String {
    match code {
        "admin_pagination_bounds" => return t!("admin-pagination-bounds"),
        "invalid_auction_command" => return t!("admin-error-invalid-auction-command"),
        "invalid_auction_state" => return t!("admin-error-invalid-auction-state"),
        "invalid_auction_intake" => return t!("admin-error-invalid-auction-intake"),
        "unsupported_equipment_source" => return t!("admin-error-unsupported-equipment-source"),
        "equipment_source_unavailable" => {
            return t!("admin-error-equipment-source-unavailable");
        }
        "auction_intake_unavailable" => return t!("admin-error-auction-intake-unavailable"),
        "seller_blacklisted" | "auction_intake_player_blacklisted" => {
            return t!("admin-error-auction-intake-player-blacklisted");
        }
        "equipment_conflict" => return t!("admin-error-equipment-conflict"),
        "invalid_credentials" => return t!("admin-error-invalid-credentials"),
        "manual_early_settlement_disabled" => {
            return t!("admin-error-manual-early-settlement-disabled");
        }
        "world_has_active_auction" => return t!("admin-error-world-has-active-auction"),
        "world_not_found" => return t!("admin-error-world-not-found"),
        "reconciliation_required" | "outcome_unknown" => {
            return t!("admin-error-reconciliation-required");
        }
        // 这三个业务码带冲突语义但不命中基础层的 `*_conflict` / `stale_*` 模式
        // （`invalid_lifecycle` 反而命中基础层 `invalid_*`），必须留在业务分支。
        "recovery_not_confirmable" | "recovery_evidence_unresolved" | "invalid_lifecycle" => {
            return t!("admin-error-conflict");
        }
        _ => {}
    }
    base_api_error_message(code, |kind| match kind {
        BaseErrorKind::Network => t!("admin-error-network"),
        BaseErrorKind::Timeout => t!("admin-error-request-timeout"),
        BaseErrorKind::InvalidResponse => t!("admin-error-invalid-response"),
        BaseErrorKind::Unauthenticated => t!("admin-error-unauthenticated"),
        BaseErrorKind::Forbidden => t!("admin-error-forbidden"),
        BaseErrorKind::NotFound => t!("admin-error-not-found"),
        BaseErrorKind::RateLimited => t!("admin-error-rate-limited"),
        BaseErrorKind::Unavailable => t!("admin-error-storage-unavailable"),
        BaseErrorKind::FeatureNotSupported => t!("admin-error-feature-not-supported"),
        BaseErrorKind::Conflict => t!("admin-error-conflict"),
        BaseErrorKind::InvalidRequest => t!("admin-error-invalid-request"),
    })
    .unwrap_or_else(|| t!("admin-error-generic"))
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
