//! 用户表单的 UTF-8 字段约束与就地错误反馈。

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::api::valid_safe_text;

pub(in crate::surface) const USER_NAME_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UserFieldIssue {
    Required,
    Name,
    ForumAccount,
    ManualKey,
    BlacklistReason,
    UsernameConflict,
}

impl UserFieldIssue {
    pub(super) fn message(self) -> String {
        match self {
            Self::Required => t!("admin-field-required"),
            Self::Name => t!("admin-users-field-username-invalid"),
            Self::ForumAccount => t!("admin-users-field-forum-account-id-invalid"),
            Self::ManualKey => t!("admin-users-field-manual-key-short"),
            Self::BlacklistReason => t!("admin-users-field-blacklist-reason-invalid"),
            Self::UsernameConflict => t!("admin-users-field-username-conflict"),
        }
    }
}

pub(super) fn username_issue(value: &str) -> Option<UserFieldIssue> {
    required_text_issue(value, USER_NAME_BYTES, UserFieldIssue::Name)
}

pub(super) fn search_name_issue(value: &str) -> Option<UserFieldIssue> {
    (!value.is_empty() && !valid_safe_text(value, USER_NAME_BYTES)).then_some(UserFieldIssue::Name)
}

pub(super) fn forum_account_issue(value: &str, required: bool) -> Option<UserFieldIssue> {
    let bytes = value.as_bytes();
    if let Some(first) = bytes.first() {
        (bytes.len() > 32 || *first == b'0' || !bytes.iter().all(u8::is_ascii_digit))
            .then_some(UserFieldIssue::ForumAccount)
    } else {
        required.then_some(UserFieldIssue::Required)
    }
}

pub(super) fn manual_key_issue(value: &str) -> Option<UserFieldIssue> {
    if value.is_empty() {
        Some(UserFieldIssue::Required)
    } else {
        (!(32..=256).contains(&value.len()) || value.chars().any(char::is_control))
            .then_some(UserFieldIssue::ManualKey)
    }
}

pub(super) fn blacklist_reason_issue(value: &str) -> Option<UserFieldIssue> {
    required_text_issue(value, 1024, UserFieldIssue::BlacklistReason)
}

fn required_text_issue(
    value: &str,
    maximum: usize,
    issue: UserFieldIssue,
) -> Option<UserFieldIssue> {
    if value.is_empty() {
        Some(UserFieldIssue::Required)
    } else {
        (!valid_safe_text(value, maximum)).then_some(issue)
    }
}

/// 提交失败由字段焦点播报；修正输入后的更新不额外触发实时朗读。
#[component]
pub(super) fn UserFieldError(id: &'static str, issue: Option<UserFieldIssue>) -> Element {
    rsx! {
        if let Some(issue) = issue {
            p { class: "admin-field-error", id, {issue.message()} }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        UserFieldIssue, blacklist_reason_issue, forum_account_issue, manual_key_issue,
        search_name_issue, username_issue,
    };

    #[test]
    fn name_and_search_match_server_byte_and_single_line_rules() {
        let limit = "界".repeat(85) + "x";
        assert_eq!(username_issue(&limit), None);
        assert_eq!(search_name_issue(&limit), None);
        assert_eq!(search_name_issue(""), None);
        assert_eq!(username_issue(""), Some(UserFieldIssue::Required));
        for value in [limit + "x", " 玩家".to_owned(), "玩\u{85}家".to_owned()] {
            assert_eq!(username_issue(&value), Some(UserFieldIssue::Name));
            assert_eq!(search_name_issue(&value), Some(UserFieldIssue::Name));
        }
    }

    #[test]
    fn account_id_distinguishes_optional_empty_from_invalid_ascii_decimal() {
        assert_eq!(forum_account_issue("", false), None);
        assert_eq!(forum_account_issue("", true), Some(UserFieldIssue::Required));
        assert_eq!(forum_account_issue(&"1".repeat(32), true), None);
        for value in ["1".repeat(33), "01".to_owned(), "１".to_owned(), "1 ".to_owned()] {
            assert_eq!(forum_account_issue(&value, false), Some(UserFieldIssue::ForumAccount));
        }
    }

    #[test]
    fn manual_key_counts_bytes_and_keeps_intentional_spaces() {
        for value in ["界".repeat(11), " ".repeat(32), "x".repeat(256)] {
            assert_eq!(manual_key_issue(&value), None);
        }
        for value in ["界".repeat(10), "x".repeat(257), "x".repeat(32) + "\t"] {
            assert_eq!(manual_key_issue(&value), Some(UserFieldIssue::ManualKey));
        }
    }

    #[test]
    fn blacklist_reason_accepts_1024_bytes_with_no_controls_or_edge_spaces() {
        let limit = "界".repeat(341) + "x";
        assert_eq!(blacklist_reason_issue(&limit), None);
        assert_eq!(blacklist_reason_issue(&"x".repeat(513)), None);
        for value in [limit + "x", "one\ntwo".to_owned(), "one ".to_owned()] {
            assert_eq!(blacklist_reason_issue(&value), Some(UserFieldIssue::BlacklistReason));
        }
    }
}
