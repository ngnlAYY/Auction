//! 凭据页面的安全录入表单。

use auction_contracts::{AdminCredentialCreateRequest, AdminCredentialLoginRequest};
use auction_web_shared::t;
use dioxus::prelude::*;
use uuid::Uuid;

use super::field_validation::{
    CredentialField, CredentialFieldError, CredentialFieldErrors, credential_field_issue_message,
    use_credential_fields,
};
use super::labels::CredentialFeedback;
use crate::api::create_admin_credential;
use crate::surface::config::{FieldLiveRegion, use_field_announcer};
use crate::surface::pending::definite_rejection;
use crate::surface::{admin_world_label, run_admin_action};

/// 只选择当前导入来源；获取成功仍仅写入所选 kind/world 的一条凭据。
#[derive(Clone, Copy, PartialEq, Eq)]
enum CredentialSource {
    Cookie,
    ForumLogin,
}

#[component]
pub fn CredentialCreateForm(reload_generation: Signal<u64>) -> Element {
    use CredentialField::{Label, Password, PlayerName, Secret, Username};

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
    let mut feedback = use_signal(|| None::<CredentialFeedback>);
    let announcer = use_field_announcer();
    let fields = use_credential_fields(announcer);

    rsx! {
        form {
            class: "admin-credentials-import",
            aria_busy: busy(),
            method: "post",
            novalidate: true,
            aria_describedby: unknown().then_some("import-unknown-help"),
            onsubmit: move |event| {
                    event.prevent_default();
                    if busy() {
                        return;
                    }
                    announcer.clear();
                    feedback.set(None);
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
                    // 已聚焦的错误字段经常驻播报区反馈，其他情况移焦后读出字段错误。
                    let mut errors = CredentialFieldErrors::default();
                    errors.set(Label, Label.issue(&label));
                    if let Some(name) = &player_name {
                        errors.set(PlayerName, PlayerName.issue(name));
                    }
                    if let Some(login) = &login {
                        errors.set(Username, Username.issue(&login.username));
                        errors.set(Password, Password.issue(&login.password));
                    } else {
                        errors.set(Secret, Secret.issue(&raw_secret));
                    }
                    fields.show(errors);
                    if let Some((field_id, issue)) = errors.first_invalid() {
                        announcer.invalid_submit(field_id, credential_field_issue_message(issue), !unknown());
                        return;
                    }
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
                        create_admin_credential(command),
                        move |result| {
                            let saved = CredentialFeedback::Success {
                                restart_required: result.restart_required,
                            };
                            announcer.announce(saved.message());
                            feedback.set(Some(saved));
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
                            fields.reset(&CredentialField::ALL);
                            reload_generation.set(reload_generation().saturating_add(1));
                        },
                        move |code| {
                            // 前置拒绝只能说明本次未写入，不能消除先前未知；不凭任意 409 解锁。
                            let remains_unknown = unknown() || !definite_rejection(&code);
                            unknown.set(remains_unknown);
                            // 可归属字段的服务端错误落到字段旁，其余保持页面级反馈。
                            let errors = CredentialFieldErrors::from_server(&code, submitted_source == CredentialSource::Cookie);
                            if let Some((field_id, issue)) = errors.first_invalid() {
                                fields.show(errors);
                                announcer.invalid_submit(field_id, credential_field_issue_message(issue), !remains_unknown);
                            } else {
                                let failure = CredentialFeedback::Error(code);
                                announcer.announce(failure.message());
                                feedback.set(Some(failure));
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
                        aria_invalid: fields.error(Label).is_some().to_string(),
                        aria_describedby: fields.error(Label).is_some().then_some("import-label-error"),
                        value: label(),
                        oninput: move |event| {
                            if busy() || unknown() { return; }
                            let value = event.value();
                            fields.edited(Label, &value);
                            label.set(value);
                            command_id_state.set(None);
                        },
                        onblur: move |_| {
                            if !busy() && !unknown() {
                                fields.blurred(Label, &label.peek());
                            }
                        },
                    }
            }
            CredentialFieldError {
                    id: "import-label-error",
                issue: fields.error(Label),
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
                        option { value: "main", {admin_world_label("main")} }
                        option { value: "isekai", {admin_world_label("isekai")} }
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
                        aria_invalid: fields.error(PlayerName).is_some().to_string(),
                        aria_describedby: if fields.error(PlayerName).is_some() { "import-player-name-help import-player-name-error" } else { "import-player-name-help" },
                        value: player_name(),
                        oninput: move |event| {
                            if busy() || unknown() { return; }
                            let value = event.value();
                            fields.edited(PlayerName, &value);
                            player_name.set(value);
                            command_id_state.set(None);
                        },
                        onblur: move |_| {
                            if !busy() && !unknown() {
                                fields.blurred(PlayerName, &player_name.peek());
                            }
                        },
                    }
                }
                p { id: "import-player-name-help", {t!("admin-credentials-player-name-help")} }
                CredentialFieldError {
                    id: "import-player-name-error",
                    issue: fields.error(PlayerName),
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
                            fields.reset(&[Secret, Username, Password]);
                            feedback.set(None);
                            announcer.clear();
                        },
                        option { value: "cookie", {t!("admin-credentials-source-cookie")} }
                        option { value: "forum_login", {t!("admin-credentials-source-login")} }
                    }
            }
            p { id: "import-source-help", {t!("admin-credentials-source-help")} }
            if source() == CredentialSource::Cookie {
            label {
                    r#for: Secret.id(),
                    {t!("admin-credentials-secret")}
                    input {
                        id: Secret.id(),
                        name: "cookie",
                        r#type: "password",
                        autocomplete: "off",
                        autocapitalize: "none",
                        "autocorrect": "off",
                        spellcheck: "false",
                        disabled: busy() || unknown(),
                        required: true,
                        aria_invalid: fields.error(Secret).is_some().to_string(),
                        aria_describedby: if fields.error(Secret).is_some() { "import-secret-help import-secret-error" } else { "import-secret-help" },
                        value: secret(),
                        oninput: move |event| {
                            if busy() || unknown() { return; }
                            let value = event.value();
                            fields.edited(Secret, &value);
                            secret.set(value);
                            command_id_state.set(None);
                        },
                        onblur: move |_| {
                            if !busy() && !unknown() {
                                fields.blurred(Secret, &secret.peek());
                            }
                        },
                    }
            }
            CredentialFieldError {
                    id: "import-secret-error",
                    issue: fields.error(Secret),
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
                        aria_invalid: fields.error(Username).is_some().to_string(),
                        aria_describedby: if fields.error(Username).is_some() { "import-login-help import-username-error" } else { "import-login-help" },
                        value: username(),
                        oninput: move |event| {
                            if busy() || unknown() { return; }
                            let value = event.value();
                            fields.edited(Username, &value);
                            username.set(value);
                            command_id_state.set(None);
                        },
                        onblur: move |_| {
                            if !busy() && !unknown() {
                                fields.blurred(Username, &username.peek());
                            }
                        },
                    }
                }
                CredentialFieldError {
                    id: "import-username-error",
                    issue: fields.error(Username),
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
                        aria_invalid: fields.error(Password).is_some().to_string(),
                        aria_describedby: if fields.error(Password).is_some() { "import-login-help import-password-error" } else { "import-login-help" },
                        value: password(),
                        oninput: move |event| {
                            if busy() || unknown() { return; }
                            let value = event.value();
                            fields.edited(Password, &value);
                            password.set(value);
                            command_id_state.set(None);
                        },
                        onblur: move |_| {
                            if !busy() && !unknown() {
                                fields.blurred(Password, &password.peek());
                            }
                        },
                    }
                }
                CredentialFieldError {
                    id: "import-password-error",
                    issue: fields.error(Password),
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
                        "action-feedback action-feedback--success"
                    } else {
                        "action-feedback action-feedback--error"
                    },
                    {feedback.message()}
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
        FieldLiveRegion { announcer }
    }
}

#[cfg(test)]
mod tests {
    use super::CredentialFieldErrors;

    #[test]
    fn credential_conflict_stays_form_level_and_a_label_error_targets_the_label() {
        for cookie_source in [true, false] {
            assert_eq!(
                CredentialFieldErrors::from_server("credential_conflict", cookie_source)
                    .first_invalid(),
                None,
            );
            assert_eq!(
                CredentialFieldErrors::from_server("invalid_credential_label", cookie_source)
                    .first_invalid()
                    .map(|(field, _)| field),
                Some("import-label"),
            );
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod form_tests {
    use std::rc::Rc;

    use auction_web_shared::i18n::{DEFAULT_ADMIN_LOCALE, build_bundle};
    use dioxus::core::{AttributeValue, ElementId, Mutation};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::CredentialCreateForm;
    use crate::surface::ADMIN_LOCALE_CATALOG;
    use crate::surface::config::test_form::{FormBrowser, dispatch, listener};

    fn import_form() -> Element {
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_ADMIN_LOCALE,
                ADMIN_LOCALE_CATALOG.default_ftl,
                &DEFAULT_ADMIN_LOCALE,
                None,
            )
        })
        .unwrap();
        let reload_generation = use_signal(|| 0);
        rsx! { CredentialCreateForm { reload_generation } }
    }

    fn mount() -> (VirtualDom, ElementId, ElementId) {
        let mut dom =
            VirtualDom::new(import_form).with_root_context(Rc::new(FormBrowser::default()));
        let initial = dom.rebuild_to_vec();
        let submit = listener(&initial.edits, "submit");
        let label = listener(&initial.edits, "input");
        assert_eq!(listener(&initial.edits, "blur"), label);
        (dom, submit, label)
    }

    fn attribute<'a>(
        edits: &'a [Mutation],
        field: ElementId,
        name: &str,
    ) -> Option<&'a AttributeValue> {
        edits.iter().find_map(|edit| match edit {
            Mutation::SetAttribute { id, name: attribute, value, .. }
                if *id == field && *attribute == name =>
            {
                Some(value)
            }
            _ => None,
        })
    }

    #[test]
    fn submitted_label_error_survives_still_invalid_edits_and_clears_on_correction() {
        let (mut dom, submit, label) = mount();
        dispatch(&dom, submit, "submit", "");
        let submitted = dom.render_immediate_to_vec();
        assert_eq!(
            attribute(&submitted.edits, label, "aria-invalid"),
            Some(&AttributeValue::Text("true".to_owned()))
        );
        assert_eq!(
            attribute(&submitted.edits, label, "aria-describedby"),
            Some(&AttributeValue::Text("import-label-error".to_owned()))
        );

        dispatch(&dom, label, "input", " ");
        let still_invalid = dom.render_immediate_to_vec();
        assert_eq!(attribute(&still_invalid.edits, label, "aria-invalid"), None);
        assert_eq!(attribute(&still_invalid.edits, label, "aria-describedby"), None);

        dispatch(&dom, label, "input", "账号一");
        let corrected = dom.render_immediate_to_vec();
        assert_eq!(
            attribute(&corrected.edits, label, "aria-invalid"),
            Some(&AttributeValue::Text("false".to_owned()))
        );
        assert_eq!(
            attribute(&corrected.edits, label, "aria-describedby"),
            Some(&AttributeValue::None)
        );
    }

    #[test]
    fn blank_cookie_keeps_its_error_through_input_blur_and_submit() {
        let mut dom =
            VirtualDom::new(import_form).with_root_context(Rc::new(FormBrowser::default()));
        let initial = dom.rebuild_to_vec();
        let submit = listener(&initial.edits, "submit");
        let label = listener(&initial.edits, "input");
        let cookie = initial
            .edits
            .iter()
            .find_map(|edit| match edit {
                Mutation::SetAttribute {
                    name: "id",
                    id,
                    value: AttributeValue::Text(value),
                    ..
                } if value == "import-secret" => Some(*id),
                _ => None,
            })
            .unwrap();
        dispatch(&dom, label, "input", "账号一");
        dispatch(&dom, submit, "submit", "");
        let submitted = dom.render_immediate_to_vec();
        assert_eq!(
            attribute(&submitted.edits, cookie, "aria-invalid"),
            Some(&AttributeValue::Text("true".to_owned())),
        );
        for (event, target, value) in
            [("input", cookie, " \t"), ("blur", cookie, ""), ("submit", submit, "")]
        {
            dispatch(&dom, target, event, value);
            let edits = dom.render_immediate_to_vec().edits;
            assert_eq!(attribute(&edits, cookie, "aria-invalid"), None, "{event}");
            assert_eq!(attribute(&edits, cookie, "aria-describedby"), None, "{event}");
        }

        dispatch(&dom, cookie, "input", "example=value");
        let corrected = dom.render_immediate_to_vec();
        assert_eq!(
            attribute(&corrected.edits, cookie, "aria-invalid"),
            Some(&AttributeValue::Text("false".to_owned())),
        );
        assert_eq!(
            attribute(&corrected.edits, cookie, "aria-describedby"),
            Some(&AttributeValue::Text("import-secret-help".to_owned())),
        );

        dispatch(&dom, cookie, "input", " \u{3000}");
        assert_eq!(attribute(&dom.render_immediate_to_vec().edits, cookie, "aria-invalid"), None);
        dispatch(&dom, cookie, "blur", "");
        assert_eq!(
            attribute(&dom.render_immediate_to_vec().edits, cookie, "aria-invalid"),
            Some(&AttributeValue::Text("true".to_owned())),
        );
    }

    #[test]
    fn label_blur_validates_only_after_input_with_the_submit_rule() {
        let (mut dom, _, label) = mount();
        dispatch(&dom, label, "blur", "");
        assert_eq!(attribute(&dom.render_immediate_to_vec().edits, label, "aria-invalid"), None);

        dispatch(&dom, label, "input", " ");
        assert_eq!(attribute(&dom.render_immediate_to_vec().edits, label, "aria-invalid"), None);
        dispatch(&dom, label, "blur", "");
        let blurred = dom.render_immediate_to_vec();
        assert_eq!(
            attribute(&blurred.edits, label, "aria-invalid"),
            Some(&AttributeValue::Text("true".to_owned()))
        );
        assert_eq!(
            attribute(&blurred.edits, label, "aria-describedby"),
            Some(&AttributeValue::Text("import-label-error".to_owned()))
        );
    }

    #[test]
    fn correcting_cookie_keeps_the_pending_label_announcement() {
        let browser = Rc::new(FormBrowser::default());
        let mut dom = VirtualDom::new(import_form).with_root_context(browser.clone());
        let edits = dom.rebuild_to_vec().edits;
        let submit = listener(&edits, "submit");
        let label = listener(&edits, "input");
        let cookie = crate::surface::realtime::text_attributes(&edits, "id")
            .find_map(|(id, value)| (value == "import-secret").then_some(id))
            .unwrap();
        dispatch(&dom, label, "input", "账号一");
        dispatch(&dom, submit, "submit", "");
        let invalid_cookie = dom.render_immediate_to_vec().edits;
        assert!(matches!(attribute(&invalid_cookie, cookie, "aria-invalid"),
            Some(AttributeValue::Text(value)) if value == "true"));

        dispatch(&dom, label, "input", " ");
        dispatch(&dom, label, "blur", "");
        let invalid_label = dom.render_immediate_to_vec().edits;
        assert!(matches!(attribute(&invalid_label, label, "aria-invalid"),
            Some(AttributeValue::Text(value)) if value == "true"));
        dispatch(&dom, cookie, "input", "example=value");
        let corrected_cookie = dom.render_immediate_to_vec().edits;
        assert!(matches!(attribute(&corrected_cookie, cookie, "aria-invalid"),
            Some(AttributeValue::Text(value)) if value == "false"));
        browser.fire_timer();
        assert!(dom.render_immediate_to_vec().edits.iter().any(|edit| matches!(edit,
            Mutation::SetText { value, .. } if value == "请填写此字段。"
        )));
    }
}
