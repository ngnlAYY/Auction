//! Admin 用户创建表单和生成 key 展示。

use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;

use super::fields::{
    UserFieldError, UserFieldIssue, forum_account_issue, manual_key_issue, username_issue,
};
use crate::api::{AdminApiFailure, create_admin_user};
use crate::surface::AdminActionFeedback;
use crate::surface::pending::{
    CommandDelivery, CommandReceipt, PendingCommandGate, PendingCommands, set_if_alive,
    try_set_if_alive,
};
use crate::surface::validation::focus_field;

/// 创建用户表单各字段的校验结果；None 表示该字段当前无错误。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct UserFieldErrors {
    username: Option<UserFieldIssue>,
    forum_account: Option<UserFieldIssue>,
    manual_key: Option<UserFieldIssue>,
}

impl UserFieldErrors {
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

/// 一次性展示的登录 Key；嵌套派生 Debug 也必须保持脱敏。
#[derive(Clone, PartialEq)]
pub(in crate::surface) struct AdminUserGeneratedKey {
    pub(super) key: String,
}

impl std::fmt::Debug for AdminUserGeneratedKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AdminUserGeneratedKey").field("key", &"[REDACTED]").finish()
    }
}

impl AdminUserGeneratedKey {
    /// 生成的 Key 保存到会话回执中，直到操作员确认交付。
    pub(in crate::surface) fn delivery(key: Option<String>, reported: bool) -> CommandDelivery {
        match key {
            Some(key) => CommandDelivery::Receipt(Some(CommandReceipt::GeneratedKey(Self { key }))),
            None => CommandDelivery::reported(reported, || None),
        }
    }
}

fn finish_create(
    username: String,
    generated_key: Option<String>,
    source_alive: bool,
    open: Signal<bool>,
    notice: Signal<Option<String>>,
) -> CommandDelivery {
    if source_alive {
        set_if_alive(open, false);
    }
    AdminUserGeneratedKey::delivery(
        generated_key,
        source_alive && try_set_if_alive(notice, Some(username)),
    )
}

#[component]
pub(super) fn AdminUserCreateForm(
    gate: PendingCommandGate,
    mut open: Signal<bool>,
    notice: Signal<Option<String>>,
) -> Element {
    let mut username = use_signal(String::new);
    let mut forum_account_id = use_signal(String::new);
    let mut key_mode = use_signal(|| "generated".to_owned());
    let mut manual_key = use_signal(String::new);
    let pending = use_context::<PendingCommands>();
    let mut feedback = use_signal(|| None::<AdminActionFeedback>);
    let mut field_errors = use_signal(UserFieldErrors::default);

    // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管。
    rsx! {
        Dialog {
            title: t!("admin-users-create-title"),
            close_label: t!("admin-users-create-close"),
            busy: pending.is_busy("create-user"),
            on_close: move |_| open.set(false),
            form {
                    class: "admin-users-create",
                    method: "post",
                    action: "/api/v1/admin/users",
                    aria_label: t!("admin-users-create-title"),
                    novalidate: true,
                    onsubmit: move |event| {
                        event.prevent_default();
                        if pending.contains("create-user") || gate.blocked() {
                            return;
                        }
                        let username = username();
                        let forum_account_id = forum_account_id();
                        let key_mode = key_mode();
                        let submitted_manual_key = manual_key();
                        let errors = UserFieldErrors {
                            username: username_issue(&username),
                            forum_account: forum_account_issue(&forum_account_id, false),
                            manual_key: if key_mode == "manual" { manual_key_issue(&submitted_manual_key) } else { None },
                        };
                        field_errors.set(errors);
                        if let Some(field_id) = errors.first_invalid_id() {
                            focus_field(field_id);
                            return;
                        }
                        feedback.set(None);
                        // 手动 Key 仅留在当前会话的工厂内存；重试复制原值，不从已清空的输入框重建。
                        let manual = (key_mode == "manual").then_some(submitted_manual_key);
                        let submitted = pending.submit_with_receipt(
                            "create-user".to_owned(),
                            format!("{} · {username}", t!("admin-users-create-open")),
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
                            move |result, source_alive| finish_create(result.user.username, result.generated_key, source_alive, open, notice),
                            move |code| {
                                let mut field_errors = field_errors;
                                if code == "user_conflict"
                                    && let Ok(mut errors) = field_errors.try_write()
                                {
                                    errors.username = Some(UserFieldIssue::UsernameConflict);
                                }
                                set_if_alive(feedback, Some(AdminActionFeedback::Error(AdminApiFailure::display_code(&code).to_owned())));
                            },
                        );
                        if submitted {
                            manual_key.set(String::new());
                        }
                    },
                    label { r#for: "create-username", {t!("admin-users-username")} span { aria_hidden: "true", " *" } }
                    input {
                        id: "create-username",
                        name: "username",
                        autocomplete: "off",
                        autocapitalize: "none",
                        spellcheck: "false",
                        disabled: pending.contains("create-user"),
                        r#type: "text",
                        required: true,
                        aria_invalid: field_errors().username.is_some().to_string(),
                        aria_describedby: field_errors().username.is_some().then_some("create-username-error"),
                        value: username(),
                        oninput: move |event| {
                            if pending.contains("create-user") { return; }
                            let value = event.value();
                            if field_errors.peek().username.is_some() {
                                field_errors.write().username = username_issue(&value);
                            }
                            username.set(value);
                        },
                    }
                    UserFieldError {
                        id: "create-username-error",
                        issue: field_errors().username,
                    }
                    label { r#for: "create-forum-account-id", {t!("admin-users-forum-account-id")} }
                    input {
                        id: "create-forum-account-id",
                        name: "forum_account_id",
                        autocomplete: "off",
                        disabled: pending.contains("create-user"),
                        r#type: "text",
                        inputmode: "numeric",
                        aria_describedby: if field_errors().forum_account.is_some() {
                            "forum-account-id-help create-forum-account-id-error"
                        } else {
                            "forum-account-id-help"
                        },
                        aria_invalid: field_errors().forum_account.is_some().to_string(),
                        value: forum_account_id(),
                        oninput: move |event| {
                            if pending.contains("create-user") { return; }
                            let value = event.value();
                            if field_errors.peek().forum_account.is_some() {
                                field_errors.write().forum_account = forum_account_issue(&value, false);
                            }
                            forum_account_id.set(value);
                        },
                    }
                    p { id: "forum-account-id-help", {t!("admin-users-forum-account-id-help")} }
                    UserFieldError {
                        id: "create-forum-account-id-error",
                        issue: field_errors().forum_account,
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
                        label { r#for: "create-key", {t!("admin-users-manual-key")} span { aria_hidden: "true", " *" } }
                        input {
                            id: "create-key",
                            name: "login_key",
                            disabled: pending.contains("create-user"),
                            r#type: "password",
                            autocomplete: "new-password",
                            required: true,
                            aria_invalid: field_errors().manual_key.is_some().to_string(),
                            aria_describedby: match field_errors().manual_key {
                                Some(UserFieldIssue::ManualKey) => "create-key-error",
                                Some(_) => "create-key-help create-key-error",
                                None => "create-key-help",
                            },
                            value: manual_key(),
                            oninput: move |event| {
                                if pending.contains("create-user") { return; }
                                let value = event.value();
                                if field_errors.peek().manual_key.is_some() {
                                    field_errors.write().manual_key = manual_key_issue(&value);
                                }
                                manual_key.set(value);
                            },
                        }
                        if field_errors().manual_key != Some(UserFieldIssue::ManualKey) {
                            p { id: "create-key-help", {t!("admin-users-field-manual-key-short")} }
                        }
                        UserFieldError {
                            id: "create-key-error",
                            issue: field_errors().manual_key,
                        }
                    }
                    button {
                        r#type: "submit",
                        class: "admin-btn-primary",
                        disabled: pending.contains("create-user") || gate.blocked(),
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
                        onclick: move |_| open.set(false),
                        {t!("admin-users-create-close")}
                    }
                }
        }
    }
}

#[component]
pub(in crate::surface) fn AdminUserGeneratedKeyDialog(
    generated: AdminUserGeneratedKey,
    on_close: EventHandler<()>,
) -> Element {
    rsx! {
        // 一次性密钥展示弹窗：外壳由共享 Dialog 接管；.admin-users-secret 内容网格
        // 样式挂在面板内的包装 div。
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::future::ready;
    use std::rc::Rc;

    use dioxus::prelude::*;

    use super::{AdminUserGeneratedKey, AdminUserGeneratedKeyDialog, finish_create};
    use crate::api::AdminApiFailure;
    use crate::surface::auctions::draft_page_tests::{
        button, click_button, has_live_text, mount_writer,
    };
    use crate::surface::pending::{
        PendingCommandPanel, PendingCommands, use_pending_command_gate,
        use_pending_commands_with_gate,
    };
    use crate::surface::realtime::TestPage;

    #[component]
    fn CreateDialogFixture(
        open: Signal<bool>,
        notice: Signal<Option<String>>,
        generate_key: bool,
    ) -> Element {
        let owner = use_context::<PendingCommands>();
        let attempts = use_hook(|| Rc::new(Cell::new(0)));
        rsx! {
            button { onclick: move |_| {
                let attempts = attempts.clone();
                assert!(owner.submit_with_receipt(
                    "create-user".to_owned(), "测试创建玩家".to_owned(),
                    move |_| {
                        let prior = attempts.replace(attempts.get() + 1);
                        ready(if prior == 0 && generate_key {
                            Err(AdminApiFailure::from_http_body(
                                503, r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
                            ).code)
                        } else { Ok(generate_key.then(|| "一次性测试值".to_owned())) })
                    },
                    move |key, live| finish_create("玩家".to_owned(), key, live, open, notice),
                    |_| {},
                ));
            }, {"创建测试用户".to_owned()} }
        }
    }

    fn mount_user_creation(generate_key: bool) -> TestPage {
        mount_writer(move || {
            let mut open = use_signal(|| true);
            let notice = use_signal(|| None);
            let reload = use_signal(|| 0);
            let gate = use_pending_command_gate();
            let owner = use_pending_commands_with_gate(reload, Some(gate), |_| true);
            rsx! {
                button { onclick: move |_| open.set(false), {"关闭创建框".to_owned()} }
                button { disabled: gate.blocked(), onclick: move |_| {}, {"开始下一次".to_owned()} }
                if open() { CreateDialogFixture { open, notice, generate_key } }
                if let Some(name) = notice() { p { "{name}" } }
                PendingCommandPanel { owner }
                if let Some((command_id, key)) = owner.generated_key() {
                    AdminUserGeneratedKeyDialog { generated: key, on_close: move |_| owner.acknowledge_key(command_id) }
                }
            }
        }).0
    }

    #[test]
    fn successful_retry_delivers_to_the_page_after_the_dialog_unmounts() {
        let mut page = mount_user_creation(true);
        click_button(&mut page, "创建测试用户");
        click_button(&mut page, "关闭创建框");
        assert!(button(&page, "创建测试用户").is_none());
        assert!(!has_live_text(&page, "一次性测试值"));

        click_button(&mut page, "原样重试");
        assert!(has_live_text(&page, "玩家"));
        assert!(has_live_text(&page, "一次性测试值"));
        assert!(button(&page, "开始下一次").unwrap().1);
        let key = AdminUserGeneratedKey { key: "一次性测试值".to_owned() };
        assert!(!format!("{key:?}").contains("一次性测试值"));

        click_button(&mut page, "关闭");
        assert!(!has_live_text(&page, "一次性测试值"));
        assert!(!button(&page, "开始下一次").unwrap().1);
    }

    #[test]
    fn a_keyless_result_does_not_open_the_key_dialog_or_hold_the_gate() {
        let mut page = mount_user_creation(false);
        click_button(&mut page, "创建测试用户");
        assert!(has_live_text(&page, "玩家"));
        assert!(button(&page, "关闭").is_none());
        assert!(!button(&page, "开始下一次").unwrap().1);
    }
}
