//! 管理端跨页面展示辅助组件。

use auction_web_shared::{
    BaseErrorKind, base_api_error_message, i18n::format_localized_time_for_language, legacy_t as t,
};
use dioxus::prelude::Memo;
#[cfg(target_arch = "wasm32")]
use dioxus::prelude::{ReadableExt, use_drop, use_effect, use_hook};
use dioxus_i18n::prelude::i18n;

/// 直接订阅当前 I18n 语言，避免 `<html lang>` 的同步 effect 使时间文案滞后一轮。
/// 时区固定为 Asia/Shanghai，格式及非法时间回退由共享层拥有。
pub fn format_admin_time(value: &str) -> String {
    format_localized_time_for_language(value, &i18n().language().to_string())
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
        "invalid_credential_secret" => return t!("admin-credentials-field-secret-invalid"),
        "credential_login_rejected" => return t!("admin-error-credential-login-rejected"),
        "credential_login_challenge" => return t!("admin-error-credential-login-challenge"),
        "credential_login_unavailable" => return t!("admin-error-credential-login-unavailable"),
        "credential_login_busy" => return t!("admin-error-credential-login-busy"),
        "manual_early_settlement_disabled" => {
            return t!("admin-error-manual-early-settlement-disabled");
        }
        "world_has_active_auction" => return t!("admin-error-world-has-active-auction"),
        "world_not_found" => return t!("admin-error-world-not-found"),
        "invalid_delivery_id" => return t!("admin-error-invalid-delivery-id"),
        "invalid_delivery_command" => return t!("admin-error-invalid-delivery-command"),
        "stale_delivery_state" => return t!("admin-error-stale-delivery-state"),
        "delivery_failed" => return t!("admin-error-delivery-failed"),
        "delivery_backlog" => return t!("admin-error-delivery-backlog"),
        "compose_not_clean" => return t!("admin-error-compose-not-clean"),
        "preflight_unproven" => return t!("admin-error-preflight-unproven"),
        "recipient_is_sender" => return t!("admin-error-recipient-is-sender"),
        "write_unknown" => return t!("admin-error-write-unknown"),
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

/// 有未保存的会话草稿时请求浏览器原生确认；同文档切页不触发此事件。
pub(super) fn use_unload_warning(_dirty: Memo<bool>) {
    #[cfg(target_arch = "wasm32")]
    {
        use std::{cell::RefCell, rc::Rc};
        use wasm_bindgen::{JsCast, closure::Closure};

        let guard = use_hook(|| Rc::new(RefCell::new(None::<UnloadWarning>)));
        let cleanup = Rc::clone(&guard);
        use_drop(move || {
            cleanup.borrow_mut().take();
        });
        use_effect(move || {
            if !_dirty() {
                guard.borrow_mut().take();
                return;
            }
            if guard.borrow().is_some() {
                return;
            }
            let Some(window) = web_sys::window() else { return };
            let callback = Closure::<dyn FnMut(web_sys::BeforeUnloadEvent)>::new(
                move |event: web_sys::BeforeUnloadEvent| {
                    if *_dirty.peek() {
                        event.prevent_default();
                        // 提示文本由浏览器控制；非空 returnValue 兼容仍依赖该属性的浏览器。
                        event.set_return_value(" ");
                    }
                },
            );
            if window
                .add_event_listener_with_callback("beforeunload", callback.as_ref().unchecked_ref())
                .is_ok()
            {
                guard.replace(Some(UnloadWarning { window, callback }));
            }
        });
    }
}

#[cfg(target_arch = "wasm32")]
struct UnloadWarning {
    window: web_sys::Window,
    callback: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::BeforeUnloadEvent)>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for UnloadWarning {
    fn drop(&mut self) {
        use wasm_bindgen::JsCast;
        let _ = self.window.remove_event_listener_with_callback(
            "beforeunload",
            self.callback.as_ref().unchecked_ref(),
        );
    }
}
