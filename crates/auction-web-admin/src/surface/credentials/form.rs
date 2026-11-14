//! 凭据页面的安全录入表单。

use auction_contracts::{AdminCredentialCreateRequest, AdminCredentialLoginRequest};
use auction_web_shared::t;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::create_admin_credential;
use crate::surface::pending::definite_rejection;
use crate::surface::validation::{FieldError, focus_field};
use crate::surface::{AdminActionFeedback, run_admin_action};

/// 凭据录入表单各字段的校验结果；None 表示该字段当前无错误。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CredentialFieldErrors {
    label: Option<CredentialFieldIssue>,
    player_name: Option<CredentialFieldIssue>,
    secret: Option<CredentialFieldIssue>,
    username: Option<CredentialFieldIssue>,
    password: Option<CredentialFieldIssue>,
}

impl CredentialFieldErrors {
    /// 是否存在任一字段错误。
    const fn any(&self) -> bool {
        self.label.is_some()
            || self.player_name.is_some()
            || self.secret.is_some()
            || self.username.is_some()
            || self.password.is_some()
    }

    /// 表单序上第一个错误字段的控件 id，用于提交失败后聚焦。
    const fn first_invalid_id(&self) -> Option<&'static str> {
        if self.label.is_some() {
            Some("import-label")
        } else if self.player_name.is_some() {
            Some("import-player-name")
        } else if self.secret.is_some() {
            Some("import-secret")
        } else if self.username.is_some() {
            Some("import-username")
        } else if self.password.is_some() {
            Some("current-password")
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
    /// 论坛账号名称超出长度或字符边界。
    UsernameInvalid,
    /// 论坛密码超出长度或字符边界。
    PasswordInvalid,
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
        CredentialFieldIssue::UsernameInvalid => t!("admin-credentials-field-username-invalid"),
        CredentialFieldIssue::PasswordInvalid => t!("admin-credentials-field-password-invalid"),
        CredentialFieldIssue::LabelConflict => t!("admin-credentials-field-label-conflict"),
    }
}

/// 只选择当前导入来源；获取成功仍仅写入所选 kind/world 的一条凭据。
#[derive(Clone, Copy, PartialEq, Eq)]
enum CredentialSource {
    Cookie,
    ForumLogin,
}

#[component]
pub fn CredentialCreateForm(reload_generation: Signal<u64>) -> Element {
    let mut kind = use_signal(|| "mail_cookie".to_owned());
    let mut world = use_signal(|| "main".to_owned());
    let mut label = use_signal(String::new);
    let mut player_name = use_signal(String::new);
    let mut secret = use_signal(String::new);
    let mut source = use_signal(|| CredentialSource::Cookie);
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut command_id_state = use_signal(|| None::<Uuid>);
    let busy = use_signal(|| false);
    // 未知结果累计在原表单；来源、密码/CK 和其他字段原样冻结，不复制到未决集合。
    // 只有用户提交才发起登录；后续前置拒绝不能解除先前未知，只有明确成功才能解锁。
    let mut unknown = use_signal(|| false);
    let mut feedback = use_signal(|| None::<AdminActionFeedback>);
    let mut field_errors = use_signal(CredentialFieldErrors::default);

    rsx! {
        form {
            class: "admin-credentials-import",
            aria_busy: busy(),
            method: "post",
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
                    let submitted_source = source();
                    let world = world_signal();
                    let label = label_signal();
                    let expected_player_name = player_name_signal();
                    let player_name = (kind == "mail_cookie" && !expected_player_name.is_empty())
                        .then_some(expected_player_name);
                    let (raw_secret, login) = match submitted_source {
                        CredentialSource::Cookie => (secret(), None),
                        CredentialSource::ForumLogin => (
                            String::new(),
                            Some(AdminCredentialLoginRequest {
                                username: username(),
                                password: password(),
                            }),
                        ),
                    };
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
                    if let Some(login) = &login {
                        if login.username.is_empty() {
                            errors.username = Some(CredentialFieldIssue::Required);
                        } else if login.username.len() > 256
                            || login.username != login.username.trim()
                            || login.username.chars().any(char::is_control)
                        {
                            errors.username = Some(CredentialFieldIssue::UsernameInvalid);
                        }
                        if login.password.is_empty() {
                            errors.password = Some(CredentialFieldIssue::Required);
                        } else if login.password.len() > 4096
                            || login.password.bytes().any(|byte| byte.is_ascii_control())
                        {
                            errors.password = Some(CredentialFieldIssue::PasswordInvalid);
                        }
                    } else if raw_secret.is_empty() {
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
                    let command = AdminCredentialCreateRequest {
                        command_id,
                        kind,
                        world,
                        label,
                        player_name,
                        secret: raw_secret,
                        login,
                    };
                    run_admin_action(
                        busy,
                        async move {
                            create_admin_credential(command).await.map(|_| ())
                        },
                        move |()| {
                            feedback.set(Some(AdminActionFeedback::Success));
                            kind_signal.set("mail_cookie".to_owned());
                            world_signal.set("main".to_owned());
                            label_signal.set(String::new());
                            player_name_signal.set(String::new());
                            secret_signal.set(String::new());
                            username.set(String::new());
                            password.set(String::new());
                            source.set(CredentialSource::Cookie);
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
                                | "credential_login_rejected" | "credential_login_challenge"
                                | "credential_login_unavailable" | "credential_login_busy"
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
                                "invalid_credential_secret"
                                    if submitted_source == CredentialSource::Cookie => {
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
                        name: "kind",
                        autocomplete: "off",
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
                        name: "label",
                        autocomplete: "off",
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
                        name: "world",
                        autocomplete: "off",
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
                        name: "player_name",
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
                    r#for: "import-source",
                    {t!("admin-credentials-source")}
                    select {
                        id: "import-source",
                        name: "source",
                        autocomplete: "off",
                        disabled: busy() || unknown(),
                        aria_describedby: "import-source-help",
                        value: match source() {
                            CredentialSource::Cookie => "cookie",
                            CredentialSource::ForumLogin => "forum_login",
                        },
                        onchange: move |event| {
                            if busy() || unknown() { return; }
                            source.set(if event.value() == "forum_login" {
                                CredentialSource::ForumLogin
                            } else {
                                CredentialSource::Cookie
                            });
                            // 已知未写入时切换来源，清除离开分支的敏感值，避免隐藏表单继续持有。
                            secret.set(String::new());
                            username.set(String::new());
                            password.set(String::new());
                            command_id_state.set(None);
                            let mut errors = field_errors.write();
                            errors.secret = None;
                            errors.username = None;
                            errors.password = None;
                            feedback.set(None);
                        },
                        option { value: "cookie", {t!("admin-credentials-source-cookie")} }
                        option { value: "forum_login", {t!("admin-credentials-source-login")} }
                    }
            }
            p { id: "import-source-help", {t!("admin-credentials-source-help")} }
            if source() == CredentialSource::Cookie {
            label {
                    r#for: "import-secret",
                    {t!("admin-credentials-secret")}
                    input {
                        id: "import-secret",
                        name: "cookie",
                        r#type: "password",
                        autocomplete: "off",
                        autocapitalize: "none",
                        "autocorrect": "off",
                        spellcheck: "false",
                        disabled: busy() || unknown(),
                        required: true,
                        aria_invalid: field_errors().secret.is_some().then_some("true"),
                        aria_describedby: "import-secret-help import-secret-error",
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
                p { id: "import-secret-help", {t!("admin-credentials-secret-help")} }
            } else {
                label {
                    r#for: "import-username",
                    {t!("admin-credentials-login-username")}
                    input {
                        id: "import-username",
                        name: "username",
                        r#type: "text",
                        autocomplete: "username",
                        autocapitalize: "none",
                        "autocorrect": "off",
                        spellcheck: "false",
                        disabled: busy() || unknown(),
                        required: true,
                        maxlength: 256,
                        aria_invalid: field_errors().username.is_some().then_some("true"),
                        aria_describedby: "import-login-help import-username-error",
                        value: username(),
                        oninput: move |event| {
                            if busy() || unknown() { return; }
                            username.set(event.value());
                            command_id_state.set(None);
                            field_errors.write().username = None;
                        },
                    }
                }
                FieldError {
                    id: "import-username-error",
                    message: field_errors().username.map(credential_field_issue_message),
                }
                label {
                    r#for: "current-password",
                    {t!("admin-credentials-login-password")}
                    input {
                        id: "current-password",
                        name: "password",
                        r#type: "password",
                        autocomplete: "current-password",
                        autocapitalize: "none",
                        "autocorrect": "off",
                        spellcheck: "false",
                        disabled: busy() || unknown(),
                        required: true,
                        maxlength: 4096,
                        aria_invalid: field_errors().password.is_some().then_some("true"),
                        aria_describedby: "import-login-help import-password-error",
                        value: password(),
                        oninput: move |event| {
                            if busy() || unknown() { return; }
                            password.set(event.value());
                            command_id_state.set(None);
                            field_errors.write().password = None;
                        },
                    }
                }
                FieldError {
                    id: "import-password-error",
                    message: field_errors().password.map(credential_field_issue_message),
                }
                p { id: "import-login-help", {t!("admin-credentials-login-help")} }
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
                    disabled: busy(),
                    if busy() { {t!("admin-action-working")} }
                    else if unknown() { {t!("admin-pending-retry")} }
                    else if source() == CredentialSource::ForumLogin {
                        {t!("admin-credentials-login-submit")}
                    }
                    else { {t!("admin-credentials-create-submit")} }
            }
        }
    }
}
