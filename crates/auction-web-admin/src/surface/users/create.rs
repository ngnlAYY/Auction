//! Admin 用户创建表单和生成 key 展示。

use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;

use crate::api::create_admin_user;
use crate::surface::AdminActionFeedback;
use crate::surface::pending::{PendingCommandGate, PendingCommands, set_if_alive};
use crate::surface::validation::{FieldError, focus_field};

/// 创建用户表单各字段的校验结果；None 表示该字段当前无错误。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct UserFieldErrors {
    username: Option<UserFieldIssue>,
    forum_account: Option<UserFieldIssue>,
    manual_key: Option<UserFieldIssue>,
}

impl UserFieldErrors {
    /// 是否存在任一字段错误。
    const fn any(&self) -> bool {
        self.username.is_some() || self.forum_account.is_some() || self.manual_key.is_some()
    }

    /// 表单序上第一个错误字段的控件 id，用于提交失败后聚焦。
    const fn first_invalid_id(&self) -> Option<&'static str> {
        if self.username.is_some() {
            Some("create-username")
        } else if self.forum_account.is_some() {
            Some("create-forum-account-id")
        } else if self.manual_key.is_some() {
            Some("create-key")
        } else {
            None
        }
    }
}

/// 字段错误的分类；渲染时翻译，保证切换语言后已展示的错误同步更新。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UserFieldIssue {
    Required,
    /// 论坛账号 ID 不是正整数文本（与服务端 `[1-9][0-9]*` 约束一致）。
    ForumAccountInvalid,
    /// 手动 Key 不足 32 个字符。
    ManualKeyShort,
    /// 服务端返回用户冲突。
    UsernameConflict,
}

fn user_field_issue_message(issue: UserFieldIssue) -> String {
    match issue {
        UserFieldIssue::Required => t!("admin-field-required"),
        UserFieldIssue::ForumAccountInvalid => t!("admin-users-field-forum-account-id-invalid"),
        UserFieldIssue::ManualKeyShort => t!("admin-users-field-manual-key-short"),
        UserFieldIssue::UsernameConflict => t!("admin-users-field-username-conflict"),
    }
}

/// 一次性展示的登录 Key；嵌套派生 Debug 也必须保持脱敏。
#[derive(Clone, PartialEq)]
pub(super) struct AdminUserGeneratedKey {
    pub(super) key: String,
}

impl std::fmt::Debug for AdminUserGeneratedKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AdminUserGeneratedKey").field("key", &"[REDACTED]").finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct AdminUserCreated {
    pub(super) username: String,
    pub(super) generated_key: Option<AdminUserGeneratedKey>,
}

#[component]
pub(super) fn AdminUserCreateForm(
    command_gate: PendingCommandGate,
    on_created: EventHandler<AdminUserCreated>,
    on_close: EventHandler<()>,
) -> Element {
    let mut username = use_signal(String::new);
    let mut forum_account_id = use_signal(String::new);
    let mut key_mode = use_signal(|| "generated".to_owned());
    let mut manual_key = use_signal(String::new);
    let pending = use_context::<PendingCommands>();
    let mut feedback = use_signal(|| None::<AdminActionFeedback>);
    let mut field_errors = use_signal(UserFieldErrors::default);

    // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管；同时修正旧版 aria_labelledby
    // 指向不存在 id（admin-users-create-title vs create-user-title）的脱节问题。
    rsx! {
        Dialog {
            title: t!("admin-users-create-title"),
            close_label: t!("admin-users-create-close"),
            busy: pending.is_busy("create-user"),
            on_close: move |_| on_close.call(()),
            form {
                    class: "admin-users-create",
                    onsubmit: move |event| {
                        event.prevent_default();
                        if pending.contains("create-user") || command_gate.blocked() {
                            return;
                        }
                        let username = username().trim().to_owned();
                        let forum_account_id = forum_account_id().trim().to_owned();
                        let key_mode = key_mode();
                        let submitted_manual_key = manual_key();
                        // 逐字段校验并定位错误；提交失败聚焦第一个错误字段。
                        let mut errors = UserFieldErrors::default();
                        if username.is_empty() {
                            errors.username = Some(UserFieldIssue::Required);
                        }
                        if !forum_account_id.is_empty()
                            && !valid_forum_account_id(&forum_account_id)
                        {
                            errors.forum_account = Some(UserFieldIssue::ForumAccountInvalid);
                        }
                        if key_mode == "manual" {
                            if submitted_manual_key.is_empty() {
                                errors.manual_key = Some(UserFieldIssue::Required);
                            } else if submitted_manual_key.len() < 32 {
                                errors.manual_key = Some(UserFieldIssue::ManualKeyShort);
                            }
                        }
                        if errors.any() {
                            field_errors.set(errors);
                            if let Some(field_id) = errors.first_invalid_id() {
                                focus_field(field_id);
                            }
                            return;
                        }
                        field_errors.set(UserFieldErrors::default());
                        feedback.set(None);
                        // 手动 Key 仅留在页级工厂内存；重试复制原值，不从已清空的输入框重建。
                        let manual = (key_mode == "manual").then_some(submitted_manual_key);
                        let submitted = pending.submit(
                            "create-user".to_owned(),
                            t!("admin-users-create-submit"),
                            move |command_id| {
                                let username = username.clone();
                                let forum_account_id = (!forum_account_id.is_empty())
                                    .then(|| forum_account_id.clone());
                                let manual = manual.clone();
                                async move {
                                    create_admin_user(
                                        command_id,
                                        username,
                                        forum_account_id,
                                        if manual.is_some() { "manual" } else { "generated" },
                                        manual,
                                    )
                                    .await
                                }
                            },
                            move |result| {
                                let generated_key =
                                    result.generated_key.map(|key| AdminUserGeneratedKey { key });
                                on_created.call(AdminUserCreated {
                                    username: result.user.username,
                                    generated_key,
                                });
                            },
                            move |code| {
                                let mut field_errors = field_errors;
                                if code == "user_conflict"
                                    && let Ok(mut errors) = field_errors.try_write()
                                {
                                    errors.username = Some(UserFieldIssue::UsernameConflict);
                                }
                                set_if_alive(feedback, Some(AdminActionFeedback::Error(code)));
                            },
                        );
                        if submitted {
                            manual_key.set(String::new());
                        }
                    },
                    label { r#for: "create-username", {t!("admin-users-username")} }
                    input {
                        id: "create-username",
                        name: "username",
                        autocomplete: "off",
                        autocapitalize: "none",
                        spellcheck: "false",
                        disabled: pending.contains("create-user"),
                        r#type: "text",
                        required: true,
                        aria_invalid: field_errors().username.is_some().then_some("true"),
                        aria_describedby: field_errors().username.is_some().then_some("create-username-error"),
                        value: username(),
                        oninput: move |event| {
                            if pending.contains("create-user") { return; }
                            username.set(event.value());
                            field_errors.write().username = None;
                        },
                    }
                    FieldError {
                        id: "create-username-error",
                        message: field_errors().username.map(user_field_issue_message),
                    }
                    label { r#for: "create-forum-account-id", {t!("admin-users-forum-account-id")} }
                    input {
                        id: "create-forum-account-id",
                        name: "forum_account_id",
                        autocomplete: "off",
                        disabled: pending.contains("create-user"),
                        r#type: "text",
                        inputmode: "numeric",
                        pattern: "[1-9][0-9]*",
                        aria_describedby: if field_errors().forum_account.is_some() {
                            "forum-account-id-help create-forum-account-id-error"
                        } else {
                            "forum-account-id-help"
                        },
                        aria_invalid: field_errors().forum_account.is_some().then_some("true"),
                        value: forum_account_id(),
                        oninput: move |event| {
                            if pending.contains("create-user") { return; }
                            forum_account_id.set(event.value());
                            field_errors.write().forum_account = None;
                        },
                    }
                    p { id: "forum-account-id-help", {t!("admin-users-forum-account-id-help")} }
                    FieldError {
                        id: "create-forum-account-id-error",
                        message: field_errors().forum_account.map(user_field_issue_message),
                    }
                    label { r#for: "create-key-mode", {t!("admin-users-key-mode")} }
                    select {
                        id: "create-key-mode",
                        name: "key_mode",
                        autocomplete: "off",
                        disabled: pending.contains("create-user"),
                        value: key_mode(),
                        onchange: move |event| {
                            if pending.contains("create-user") { return; }
                            key_mode.set(event.value());
                            manual_key.set(String::new());
                            field_errors.write().manual_key = None;
                        },
                        option { value: "generated", {t!("admin-users-key-generated")} }
                        option { value: "manual", {t!("admin-users-key-manual")} }
                    }
                    if key_mode() == "manual" {
                        label { r#for: "create-key", {t!("admin-users-manual-key")} }
                        input {
                            id: "create-key",
                            name: "login_key",
                            disabled: pending.contains("create-user"),
                            r#type: "password",
                            autocomplete: "new-password",
                            minlength: "32",
                            required: true,
                            aria_invalid: field_errors().manual_key.is_some().then_some("true"),
                            aria_describedby: field_errors().manual_key.is_some().then_some("create-key-error"),
                            value: manual_key(),
                            oninput: move |event| {
                                if pending.contains("create-user") { return; }
                                manual_key.set(event.value());
                                field_errors.write().manual_key = None;
                            },
                        }
                        FieldError {
                            id: "create-key-error",
                            message: field_errors().manual_key.map(user_field_issue_message),
                        }
                    }
                    button {
                        r#type: "submit",
                        class: "admin-btn-primary",
                        disabled: pending.contains("create-user") || command_gate.blocked(),
                        {t!("admin-users-create-submit")}
                    }
                }
                if let Some(AdminActionFeedback::Error(code)) = feedback() {
                    p { role: "alert", {AdminActionFeedback::Error(code).message(String::new())} }
                }
                div { class: "admin-shell__dialog-actions",
                    button {
                        r#type: "button",
                        disabled: pending.is_busy("create-user"),
                        onclick: move |_| on_close.call(()),
                        {t!("admin-users-create-close")}
                    }
                }
        }
    }
}

#[component]
pub(super) fn AdminUserGeneratedKeyDialog(
    generated: AdminUserGeneratedKey,
    on_close: EventHandler<()>,
) -> Element {
    rsx! {
        // 一次性密钥展示弹窗：外壳由共享 Dialog 接管；.admin-users-secret 内容网格
        // 样式移到面板内的包装 div。
        Dialog {
            title: t!("admin-users-generated-key-title"),
            close_label: t!("admin-users-generated-key-dismiss"),
            on_close: move |_| on_close.call(()),
            div { class: "admin-users-secret",
                p { {t!("admin-users-generated-key-warning")} }
                code { {generated.key} }
                div { class: "admin-shell__dialog-actions",
                    button {
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("admin-users-generated-key-dismiss")}
                    }
                }
            }
        }
    }
}

/// 论坛账号 ID 校验：与输入框 `pattern="[1-9][0-9]*"` 一致，首位非零的全数字文本。
fn valid_forum_account_id(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_digit() && first != b'0' && bytes.all(|byte| byte.is_ascii_digit())
}
