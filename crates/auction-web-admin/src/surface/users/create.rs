//! Admin 用户创建表单和生成 key 展示。

use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;

use super::fields::{
    UserFieldError, UserFieldIssue, forum_account_issue, manual_key_issue, username_issue,
};
use crate::api::{AdminApiFailure, create_admin_user};
use crate::surface::AdminActionFeedback;
use crate::surface::pending::{PendingCommandGate, PendingCommands, set_if_alive};
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
pub(super) struct AdminUserGeneratedKey {
    pub(super) key: String,
}

impl std::fmt::Debug for AdminUserGeneratedKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AdminUserGeneratedKey").field("key", &"[REDACTED]").finish()
    }
}

/// 页级持有一次性 Key，创建弹窗或表格卸载后仍能交付；确认展示后立即清除。
#[derive(Clone, Copy, PartialEq)]
pub(super) struct UserKeyDelivery {
    pub(super) gate: PendingCommandGate,
    pub(super) generated: Signal<Option<AdminUserGeneratedKey>>,
}

impl UserKeyDelivery {
    pub(super) fn deliver(mut self, key: Option<String>) {
        if let Some(key) = key {
            self.gate.hold_result();
            self.generated.set(Some(AdminUserGeneratedKey { key }));
        }
    }

    pub(super) fn acknowledge(mut self) {
        self.generated.set(None);
        self.gate.acknowledge();
    }
}

fn finish_create(
    username: String,
    generated_key: Option<String>,
    delivery: UserKeyDelivery,
    mut open: Signal<bool>,
    mut notice: Signal<Option<String>>,
) {
    delivery.deliver(generated_key);
    notice.set(Some(username));
    open.set(false);
}

#[component]
pub(super) fn AdminUserCreateForm(
    delivery: UserKeyDelivery,
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
                        if pending.contains("create-user") || delivery.gate.blocked() {
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
                            move |result| finish_create(result.user.username, result.generated_key, delivery, open, notice),
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
                        disabled: pending.contains("create-user") || delivery.gate.blocked(),
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
pub(super) fn AdminUserGeneratedKeyDialog(
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
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    use dioxus::dioxus_core::NoOpMutations;
    use dioxus::prelude::*;

    use super::{UserKeyDelivery, finish_create};
    use crate::surface::pending::use_pending_command_gate;

    #[derive(Default)]
    struct PageControls {
        open: Cell<Option<Signal<bool>>>,
        notice: Cell<Option<Signal<Option<String>>>>,
        delivery: Cell<Option<UserKeyDelivery>>,
        dialog_dropped: Cell<bool>,
        completion: RefCell<Option<Box<dyn FnOnce()>>>,
    }

    #[component]
    fn DialogScope(
        delivery: UserKeyDelivery,
        open: Signal<bool>,
        notice: Signal<Option<String>>,
    ) -> Element {
        let control = consume_context::<Rc<PageControls>>();
        let page = control.clone();
        use_hook(move || {
            *page.completion.borrow_mut() = Some(Box::new(move || {
                finish_create("玩家".into(), Some("一次性测试值".into()), delivery, open, notice);
            }));
        });
        use_drop(move || control.dialog_dropped.set(true));
        rsx! { div {} }
    }

    fn user_page() -> Element {
        let control = consume_context::<Rc<PageControls>>();
        let open = use_signal(|| true);
        let notice = use_signal(|| None);
        let delivery =
            UserKeyDelivery { gate: use_pending_command_gate(), generated: use_signal(|| None) };
        control.open.set(Some(open));
        control.notice.set(Some(notice));
        control.delivery.set(Some(delivery));
        rsx! { if open() { DialogScope { delivery, open, notice } } }
    }

    #[test]
    fn successful_retry_delivers_to_the_page_after_the_dialog_unmounts() {
        let control = Rc::new(PageControls::default());
        let mut dom = VirtualDom::new(user_page).with_root_context(control.clone());
        dom.rebuild_in_place();
        let mut open = control.open.get().unwrap();
        let notice = control.notice.get().unwrap();
        let delivery = control.delivery.get().unwrap();

        dom.in_runtime(|| open.set(false));
        dom.render_immediate(&mut NoOpMutations);
        assert!(control.dialog_dropped.get());
        assert!(delivery.generated.peek().is_none());

        let complete = control.completion.borrow_mut().take().unwrap();
        dom.in_runtime(complete);
        assert_eq!(notice.peek().as_deref(), Some("玩家"));
        assert!(delivery.gate.blocked());
        assert_eq!(
            delivery.generated.peek().as_ref().map(|result| result.key.as_str()),
            Some("一次性测试值")
        );
        let debug = format!("{:?}", delivery.generated.peek().as_ref());
        assert!(!debug.contains("一次性测试值"));
        dom.in_runtime(|| delivery.acknowledge());
        assert!(delivery.generated.peek().is_none());
        assert!(!delivery.gate.blocked());
    }

    #[test]
    fn a_keyless_result_does_not_open_the_key_dialog_or_hold_the_gate() {
        let control = Rc::new(PageControls::default());
        let mut dom = VirtualDom::new(user_page).with_root_context(control.clone());
        dom.rebuild_in_place();
        let delivery = control.delivery.get().unwrap();
        dom.in_runtime(|| delivery.deliver(None));
        assert!(delivery.generated.peek().is_none());
        assert!(!delivery.gate.blocked());
    }
}
