//! 规则草稿、反馈和 locale 边界辅助。

use auction_web_shared::legacy_t as t;

use crate::api::{AdminRules, AdminRulesCandidate};
use crate::app::surface::admin_api_error_message;

use super::model::RuleNotice;

pub(super) const DEFAULT_RULES_LOCALE: &str = "en";

pub(super) fn notice_text(notice: &RuleNotice) -> String {
    match notice {
        RuleNotice::Validated => t!("admin-rules-validated"),
        RuleNotice::Published => t!("admin-rules-success"),
        RuleNotice::ActionError(code) => admin_api_error_message(code),
        RuleNotice::Conflict => t!("admin-rules-conflict"),
    }
}

pub(super) fn to_draft(document: &AdminRules, requested_locale: &str) -> AdminRulesCandidate {
    AdminRulesCandidate {
        locale: requested_locale.to_owned(),
        title: document.title.clone(),
        markdown: document.markdown.clone(),
    }
}

pub(super) fn missing_rules_document(locale: String) -> AdminRules {
    AdminRules {
        locale,
        title: String::new(),
        markdown: String::new(),
        published_by: t!("value-empty"),
        revision: 0,
        published_at: String::new(),
    }
}

pub(super) fn draft_signature(draft: &AdminRulesCandidate) -> String {
    format!("{}\u{1f}{}\u{1f}{}", draft.locale, draft.title, draft.markdown)
}

pub(super) fn valid_locale(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 35
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value.chars().all(|character| character.is_alphanumeric() || character == '-')
}
