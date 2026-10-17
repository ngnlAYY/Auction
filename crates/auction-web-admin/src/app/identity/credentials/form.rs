//! 凭据页面的安全录入表单。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::create_admin_credential;
use crate::app::surface::pending::definite_rejection;
use crate::app::surface::validation::{FieldError, focus_field};
use crate::app::surface::{AdminActionFeedback, run_admin_action};

/// 凭据录入表单各字段的校验结果；None 表示该字段当前无错误。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CredentialFieldErrors {
    label: Option<CredentialFieldIssue>,
    player_name: Option<CredentialFieldIssue>,
    secret: Option<CredentialFieldIssue>,
}

impl CredentialFieldErrors {
    /// 是否存在任一字段错误。
    const fn any(&self) -> bool {
        self.label.is_some() || self.player_name.is_some() || self.secret.is_some()
    }

    /// 表单序上第一个错误字段的控件 id，用于提交失败后聚焦。
    const fn first_invalid_id(&self) -> Option<&'static str> {
        if self.label.is_some() {
            Some("import-label")
        } else if self.player_name.is_some() {
            Some("import-player-name")
        } else if self.secret.is_some() {
            Some("import-secret")
        } else {
            None
        }
    }
}

/// 字段错误的分类；渲染时翻译，保证切换语言后已展示的错误同步更新。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CredentialFieldIssue {
    Required,
    /// 服务端返回标签缺失或无效。
    LabelInvalid,
    /// 预期完整玩家名称不符合原文边界。
    PlayerNameInvalid,
    /// 服务端返回凭据内容无效。
    SecretInvalid,
    /// 服务端返回凭据冲突（同 world/kind 下标签重复）。
    LabelConflict,
}

fn credential_field_issue_message(issue: CredentialFieldIssue) -> String {
    match issue {
        CredentialFieldIssue::Required => t!("admin-field-required"),
        CredentialFieldIssue::LabelInvalid => t!("admin-credentials-field-label-invalid"),
        CredentialFieldIssue::PlayerNameInvalid => {
            t!("admin-credentials-field-player-name-invalid")
        }
        CredentialFieldIssue::SecretInvalid => t!("admin-credentials-field-secret-invalid"),
        CredentialFieldIssue::LabelConflict => t!("admin-credentials-field-label-conflict"),
    }
}

#[component]
pub fn CredentialCreateForm(reload_generation: Signal<u64>) -> Element {
    let mut kind = use_signal(|| "mail_cookie".to_owned());
    let mut world = use_signal(|| "main".to_owned());
    let mut label = use_signal(String::new);
    let mut player_name = use_signal(String::new);
    let mut secret = use_signal(String::new);
    let mut command_id_state = use_signal(|| None::<Uuid>);
    let busy = use_signal(|| false);
    // 未知结果累计在原表单；Cookie 不复制到未决集合，只有明确成功才能解除锁定。
    let mut unknown = use_signal(|| false);
    let mut feedback = use_signal(|| None::<AdminActionFeedback>);
    let mut field_errors = use_signal(CredentialFieldErrors::default);

    rsx! {
        form {
            class: "admin-credentials-import",
            aria_busy: busy(),
            aria_describedby: unknown().then_some("import-unknown-help"),
            onsubmit: move |event| {
                    event.prevent_default();
                    if busy() {
                        return;
                    }
                    let mut kind_signal = kind;
                    let mut world_signal = world;
                    let mut label_signal = label;
                    let mut player_name_signal = player_name;
                    let mut secret_signal = secret;
                    let kind = kind_signal();
                    let world = world_signal();
                    let label = label_signal();
                    let expected_player_name = player_name_signal();
                    let player_name = (kind == "mail_cookie" && !expected_player_name.is_empty())
                        .then_some(expected_player_name);
                    let raw_secret = secret();
                    // 逐字段校验并定位错误；提交失败聚焦第一个错误字段。
                    let mut errors = CredentialFieldErrors::default();
                    if label.trim().is_empty() {
                        errors.label = Some(CredentialFieldIssue::Required);
                    }
                    if player_name.as_deref().is_some_and(|name| {
                        name != name.trim() || name.len() > 256 || name.chars().any(char::is_control)
                    }) {
                        errors.player_name = Some(CredentialFieldIssue::PlayerNameInvalid);
                    }
                    if raw_secret.is_empty() {
                        errors.secret = Some(CredentialFieldIssue::Required);
                    }
                    if errors.any() {
                        field_errors.set(errors);
                        if let Some(field_id) = errors.first_invalid_id() {
                            focus_field(field_id);
                        }
                        return;
                    }
                    field_errors.set(CredentialFieldErrors::default());
                    feedback.set(None);
                    let command_id = match command_id_state() {
                        Some(command_id) => command_id,
                        None => {
                            let command_id = Uuid::new_v4();
                            command_id_state.set(Some(command_id));
                            command_id
                        }
                    };
                    run_admin_action(
                        busy,
                        async move {
                            create_admin_credential(command_id, kind, world, label, player_name, raw_secret)
                                .await
                                .map(|_| ())
                        },
                        move |()| {
                            feedback.set(Some(AdminActionFeedback::Success));
                            kind_signal.set("mail_cookie".to_owned());
                            world_signal.set("main".to_owned());
                            label_signal.set(String::new());
                            player_name_signal.set(String::new());
                            secret_signal.set(String::new());
                            command_id_state.set(None);
                            unknown.set(false);
                            field_errors.set(CredentialFieldErrors::default());
                            reload_generation.set(reload_generation().saturating_add(1));
                        },
                        move |code| {
                            // 前置拒绝只能说明本次未写入，不能消除先前未知；不凭任意 409 解锁。
                            let rejected = definite_rejection(&code) || matches!(code.as_str(),
                                "invalid_credential_kind" | "invalid_credential_world"
                                | "invalid_credential_request" | "invalid_credential_label"
                                | "invalid_credential_player_name" | "invalid_credential_secret"
                                | "credential_conflict"
                            );
                            unknown.set(unknown() || !rejected);
                            // 可归属字段的服务端错误落到字段旁，其余保持页面级反馈。
                            match code.as_str() {
                                "invalid_credential_label" => {
                                    field_errors.write().label =
                                        Some(CredentialFieldIssue::LabelInvalid);
                                    focus_field("import-label");
                                }
                                "invalid_credential_player_name" => {
                                    field_errors.write().player_name =
                                        Some(CredentialFieldIssue::PlayerNameInvalid);
                                    focus_field("import-player-name");
                                }
                                "invalid_credential_secret" => {
                                    field_errors.write().secret =
                                        Some(CredentialFieldIssue::SecretInvalid);
                                    focus_field("import-secret");
                                }
                                "credential_conflict" => {
                                    field_errors.write().label =
                                        Some(CredentialFieldIssue::LabelConflict);
                                    focus_field("import-label");
                                }
                                _ => feedback.set(Some(AdminActionFeedback::Error(code))),
                            }
                        },
                    );
                },
            h2 { {t!("admin-credentials-create-title")} }
            label {
                    r#for: "import-kind",
                    {t!("admin-credentials-kind")}
                    select {
                        id: "import-kind",
                        disabled: busy() || unknown(),
                        value: kind(),
                        onchange: move |event| {
                            if busy() || unknown() { return; }
                            kind.set(event.value());
                            command_id_state.set(None);
                        },
                        option { value: "mail_cookie", {t!("admin-credentials-kind-mail")} }
                        option { value: "forum_cookie", {t!("admin-credentials-kind-forum")} }
                    }
            }
            label {
                    r#for: "import-label",
                    {t!("admin-credentials-label")}
                    input {
                        id: "import-label",
                        r#type: "text",
                        disabled: busy() || unknown(),
                        required: true,
                        aria_invalid: field_errors().label.is_some().then_some("true"),
                        aria_describedby: field_errors().label.is_some().then_some("import-label-error"),
                        value: label(),
                        oninput: move |event| {
                            if busy() || unknown() { return; }
                            label.set(event.value());
                            command_id_state.set(None);
                            field_errors.write().label = None;
                        },
                    }
            }
            FieldError {
                    id: "import-label-error",
                    message: field_errors().label.map(credential_field_issue_message),
            }
            label {
                    r#for: "import-world",
                    {t!("admin-credentials-world")}
                    select {
                        id: "import-world",
                        disabled: busy() || unknown(),
                        value: world(),
                        onchange: move |event| {
                            if busy() || unknown() { return; }
                            world.set(event.value());
                            command_id_state.set(None);
                        },
                        option { value: "main", {t!("admin-credentials-world-main")} }
                        option { value: "isekai", {t!("admin-credentials-world-isekai")} }
                        option { value: "shared", {t!("admin-credentials-world-shared")} }
                    }
            }
            if kind() == "mail_cookie" {
                label {
                    r#for: "import-player-name",
                    {t!("admin-credentials-player-name")}
                    input {
                        id: "import-player-name",
                        r#type: "text",
                        disabled: busy() || unknown(),
                        autocomplete: "off",
                        autocapitalize: "none",
                        "autocorrect": "off",
                        spellcheck: "false",
                        aria_invalid: field_errors().player_name.is_some().then_some("true"),
                        aria_describedby: "import-player-name-help import-player-name-error",
                        value: player_name(),
                        oninput: move |event| {
                            if busy() || unknown() { return; }
                            player_name.set(event.value());
                            command_id_state.set(None);
                            field_errors.write().player_name = None;
                        },
                    }
                }
                p { id: "import-player-name-help", {t!("admin-credentials-player-name-help")} }
                FieldError {
                    id: "import-player-name-error",
                    message: field_errors().player_name.map(credential_field_issue_message),
                }
            }
            label {
                    r#for: "import-secret",
                    {t!("admin-credentials-secret")}
                    input {
                        id: "import-secret",
                        r#type: "password",
                        autocomplete: "off",
                        disabled: busy() || unknown(),
                        required: true,
                        aria_invalid: field_errors().secret.is_some().then_some("true"),
                        aria_describedby: field_errors().secret.is_some().then_some("import-secret-error"),
                        value: secret(),
                        oninput: move |event| {
                            if busy() || unknown() { return; }
                            secret.set(event.value());
                            command_id_state.set(None);
                            field_errors.write().secret = None;
                        },
                    }
            }
            FieldError {
                    id: "import-secret-error",
                    message: field_errors().secret.map(credential_field_issue_message),
            }
            if unknown() {
                p { id: "import-unknown-help", role: "status",
                    {t!("admin-credentials-import-unknown")}
                }
            }
            if let Some(feedback) = feedback() {
                    p {
                        class: if feedback.is_success() {
                            "admin-shell__inline-ok"
                        } else {
                            "admin-shell__inline-error"
                        },
                        role: if feedback.is_success() { "status" } else { "alert" },
                        {feedback.message(t!("admin-credentials-action-success"))}
                    }
            }
            button {
                    r#type: "submit",
                    class: "admin-btn-primary",
                    disabled: busy() || label().trim().is_empty() || secret().is_empty(),
                    if busy() { {t!("admin-action-working")} }
                    else if unknown() { {t!("admin-pending-retry")} }
                    else { {t!("admin-credentials-create-submit")} }
            }
        }
    }
}
