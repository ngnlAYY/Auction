//! 凭据表单字段在提交、编辑和离焦时共享的校验与反馈。

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::api::valid_safe_text;
use crate::surface::config::FieldAnnouncer;

#[derive(Clone, Copy)]
pub(super) enum CredentialField {
    Label,
    PlayerName,
    Secret,
    Username,
    Password,
}

impl CredentialField {
    pub(super) const ALL: [Self; 5] =
        [Self::Label, Self::PlayerName, Self::Secret, Self::Username, Self::Password];

    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Label => "import-label",
            Self::PlayerName => "import-player-name",
            Self::Secret => "import-secret",
            Self::Username => "import-username",
            Self::Password => "current-password",
        }
    }

    pub(super) fn issue(self, value: &str) -> Option<CredentialFieldIssue> {
        match self {
            Self::Label | Self::Secret if value.trim().is_empty() => {
                Some(CredentialFieldIssue::Required)
            }
            Self::Label if !valid_safe_text(value, 256) => Some(CredentialFieldIssue::LabelInvalid),
            Self::PlayerName if !value.is_empty() && !valid_safe_text(value, 256) => {
                Some(CredentialFieldIssue::PlayerNameInvalid)
            }
            Self::Username | Self::Password if value.is_empty() => {
                Some(CredentialFieldIssue::Required)
            }
            Self::Username if !valid_safe_text(value, 256) => {
                Some(CredentialFieldIssue::UsernameInvalid)
            }
            Self::Password
                if value.len() > 4096 || value.bytes().any(|byte| byte.is_ascii_control()) =>
            {
                Some(CredentialFieldIssue::PasswordInvalid)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct CredentialFieldErrors([Option<CredentialFieldIssue>; CredentialField::ALL.len()]);

impl CredentialFieldErrors {
    fn get(self, field: CredentialField) -> Option<CredentialFieldIssue> {
        self.0[field as usize]
    }

    pub(super) fn set(&mut self, field: CredentialField, issue: Option<CredentialFieldIssue>) {
        self.0[field as usize] = issue;
    }

    /// 表单序上第一个错误字段及其文案类型。
    pub(super) fn first_invalid(self) -> Option<(&'static str, CredentialFieldIssue)> {
        CredentialField::ALL
            .into_iter()
            .find_map(|field| self.get(field).map(|issue| (field.id(), issue)))
    }

    /// 只有服务端明确归属字段的错误才移动焦点。
    pub(super) fn from_server(code: &str, cookie_source: bool) -> Self {
        let mut errors = Self::default();
        match code {
            "invalid_credential_label" => {
                errors.set(CredentialField::Label, Some(CredentialFieldIssue::LabelInvalid));
            }
            "invalid_credential_player_name" => {
                errors.set(
                    CredentialField::PlayerName,
                    Some(CredentialFieldIssue::PlayerNameInvalid),
                );
            }
            "invalid_credential_secret" if cookie_source => {
                errors.set(CredentialField::Secret, Some(CredentialFieldIssue::SecretInvalid));
            }
            _ => {}
        }
        errors
    }
}

#[derive(Clone, Copy)]
pub(super) struct CredentialFieldFeedback {
    errors: Signal<CredentialFieldErrors>,
    edited: Signal<[bool; CredentialField::ALL.len()]>,
    announced: Signal<[Option<CredentialFieldIssue>; CredentialField::ALL.len()]>,
    announcer: FieldAnnouncer,
}

pub(super) fn use_credential_fields(announcer: FieldAnnouncer) -> CredentialFieldFeedback {
    CredentialFieldFeedback {
        errors: use_signal(CredentialFieldErrors::default),
        edited: use_signal(|| [false; CredentialField::ALL.len()]),
        announced: use_signal(|| [None; CredentialField::ALL.len()]),
        announcer,
    }
}

impl CredentialFieldFeedback {
    pub(super) fn error(self, field: CredentialField) -> Option<CredentialFieldIssue> {
        self.errors.read().get(field)
    }

    pub(super) fn show(mut self, errors: CredentialFieldErrors) {
        self.errors.set(errors);
        // 提交失败已就地展示并聚焦播报；记录后离焦不重复播报同一条。
        let mut announced = self.announced.write();
        for field in CredentialField::ALL {
            if let Some(issue) = errors.get(field) {
                announced[field as usize] = Some(issue);
            }
        }
    }

    pub(super) fn edited(mut self, field: CredentialField, value: &str) {
        self.edited.write()[field as usize] = true;
        let previous = self.errors.peek().get(field);
        if previous.is_some() {
            let issue = field.issue(value);
            if issue != previous {
                self.errors.write().set(field, issue);
                self.announcer.clear_field(field.id());
            }
        }
    }

    pub(super) fn blurred(mut self, field: CredentialField, value: &str) {
        // 首次交互到离焦才提示；同一条消息只播报一次，换成新消息时仍会播报。
        if !self.edited.peek()[field as usize] {
            return;
        }
        let issue = field.issue(value);
        if self.errors.peek().get(field) != issue {
            self.errors.write().set(field, issue);
        }
        let mut announced = self.announced.write();
        if announced[field as usize] != issue {
            announced[field as usize] = issue;
            if let Some(issue) = issue {
                self.announcer.announce_on_blur(field.id(), credential_field_issue_message(issue));
            }
        }
    }

    pub(super) fn reset(mut self, fields: &[CredentialField]) {
        let mut errors = self.errors.write();
        let mut edited = self.edited.write();
        let mut announced = self.announced.write();
        for &field in fields {
            errors.set(field, None);
            edited[field as usize] = false;
            announced[field as usize] = None;
        }
    }
}

/// 字段错误的分类；渲染时翻译，保证切换语言后已展示的错误同步更新。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CredentialFieldIssue {
    Required,
    /// 标签不符合长度或字符边界。
    LabelInvalid,
    /// 预期完整玩家名称不符合原文边界。
    PlayerNameInvalid,
    /// 服务端返回凭据内容无效。
    SecretInvalid,
    /// 论坛账号名称超出长度或字符边界。
    UsernameInvalid,
    /// 论坛密码超出长度或字符边界。
    PasswordInvalid,
}

pub(super) fn credential_field_issue_message(issue: CredentialFieldIssue) -> String {
    match issue {
        CredentialFieldIssue::Required => t!("admin-field-required"),
        CredentialFieldIssue::LabelInvalid => t!("admin-credentials-field-label-invalid"),
        CredentialFieldIssue::PlayerNameInvalid => {
            t!("admin-credentials-field-player-name-invalid")
        }
        CredentialFieldIssue::SecretInvalid => t!("admin-credentials-field-secret-invalid"),
        CredentialFieldIssue::UsernameInvalid => t!("admin-credentials-field-username-invalid"),
        CredentialFieldIssue::PasswordInvalid => t!("admin-credentials-field-password-invalid"),
    }
}

#[component]
pub(super) fn CredentialFieldError(
    #[props(into)] id: String,
    issue: Option<CredentialFieldIssue>,
) -> Element {
    rsx! {
        if let Some(issue) = issue {
            p { class: "admin-field-error", id, {credential_field_issue_message(issue)} }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CredentialField, CredentialFieldIssue};

    #[test]
    fn each_text_field_uses_its_own_submission_rules() {
        use CredentialField::{Label, Password, PlayerName, Secret, Username};
        use CredentialFieldIssue::{
            LabelInvalid, PasswordInvalid, PlayerNameInvalid, Required, UsernameInvalid,
        };

        for (field, valid, invalid, issue) in [
            (Label, "账号一", " ", Required),
            (Label, "账号一", " 账号一", LabelInvalid),
            (PlayerName, "", " ", PlayerNameInvalid),
            (PlayerName, "Player Name", "Player Name\n", PlayerNameInvalid),
            (Secret, "example=value", " \u{3000}", Required),
            (Username, "Forum User", "Forum User ", UsernameInvalid),
            (Password, " Password ", "", Required),
            (Password, "Password", "bad\npassword", PasswordInvalid),
        ] {
            assert_eq!(field.issue(valid), None);
            assert_eq!(field.issue(invalid), Some(issue));
        }
        assert_eq!(Label.issue(&"名".repeat(86)), Some(LabelInvalid));
        assert_eq!(Username.issue(&"名".repeat(86)), Some(UsernameInvalid));
        assert_eq!(Password.issue(&"密".repeat(1366)), Some(PasswordInvalid));
    }
}
