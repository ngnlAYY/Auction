//! 凭据页面的安全标签映射辅助。

use auction_web_shared::t;

use crate::surface::{admin_api_error_message, admin_world_label};

/// 凭据写入反馈使用服务端的重启标志和稳定错误码。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CredentialFeedback {
    Success { restart_required: bool },
    Error(String),
}

impl CredentialFeedback {
    pub(super) const fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    pub(super) fn message(&self) -> String {
        match self {
            Self::Success { restart_required: true } => t!("admin-credentials-action-restart"),
            Self::Success { restart_required: false } => t!("admin-credentials-action-success"),
            Self::Error(code) => match code.as_str() {
                // 同类 active、标签、指纹和命令负载冲突共用一码，不能归到单个输入框。
                "credential_conflict" => t!("admin-credentials-action-conflict"),
                "stale_credential_state" => t!("admin-credentials-action-stale"),
                "invalid_credential_label" => t!("admin-credentials-field-label-invalid"),
                _ => admin_api_error_message(code),
            },
        }
    }
}

pub fn credential_kind_label(kind: &str) -> String {
    match kind {
        "mail_cookie" => t!("admin-credentials-kind-mail"),
        "forum_cookie" => t!("admin-credentials-kind-forum"),
        _ => kind.to_owned(),
    }
}

/// 凭据作用域除业务世界外还有跨世界共用的 `shared`；业务世界名统一委托 [`admin_world_label`]。
pub fn credential_world_label(world: &str) -> String {
    match world {
        "shared" => t!("admin-credentials-world-shared"),
        _ => admin_world_label(world),
    }
}

pub fn credential_status_label(status: &str) -> String {
    match status {
        "active" => t!("admin-credentials-status-active"),
        "disabled" => t!("admin-credentials-status-disabled"),
        "invalid" => t!("admin-credentials-status-invalid"),
        _ => status.to_owned(),
    }
}

pub fn credential_status_class(status: &str) -> &'static str {
    match status {
        "active" => "admin-credentials__status admin-credentials__status--active",
        "disabled" => "admin-credentials__status admin-credentials__status--disabled",
        "invalid" => "admin-credentials__status admin-credentials__status--invalid",
        _ => "admin-credentials__status",
    }
}

#[cfg(test)]
mod tests {
    use auction_web_shared::{
        i18n::{DEFAULT_ADMIN_LOCALE, build_bundle},
        t,
    };
    use dioxus::{core::ScopeId, prelude::*};
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::CredentialFeedback;
    use crate::surface::ADMIN_LOCALE_CATALOG;

    #[component]
    fn FeedbackHarness() -> Element {
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_ADMIN_LOCALE,
                ADMIN_LOCALE_CATALOG.default_ftl,
                &DEFAULT_ADMIN_LOCALE,
                None,
            )
        })
        .unwrap();
        VNode::empty()
    }

    #[test]
    fn writes_report_restart_from_the_server_flag() {
        let mut dom = VirtualDom::new(FeedbackHarness);
        dom.rebuild_in_place();
        dom.in_scope(ScopeId::APP, || {
            assert_eq!(
                CredentialFeedback::Success { restart_required: true }.message(),
                t!("admin-credentials-action-restart"),
            );
            assert_eq!(
                CredentialFeedback::Success { restart_required: false }.message(),
                t!("admin-credentials-action-success"),
            );
        });
    }

    #[test]
    fn credential_conflict_and_stale_version_receive_distinct_feedback() {
        let mut dom = VirtualDom::new(FeedbackHarness);
        dom.rebuild_in_place();
        dom.in_scope(ScopeId::APP, || {
            assert_eq!(
                CredentialFeedback::Error("credential_conflict".to_owned()).message(),
                t!("admin-credentials-action-conflict"),
            );
            assert_eq!(
                CredentialFeedback::Error("stale_credential_state".to_owned()).message(),
                t!("admin-credentials-action-stale"),
            );
        });
    }
}
