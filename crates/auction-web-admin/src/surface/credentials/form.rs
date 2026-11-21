//! 凭据页面的安全录入表单。

use std::rc::Rc;

use auction_contracts::{AdminCredentialCreateRequest, AdminCredentialLoginRequest};
use auction_web_shared::t;
use dioxus::{core::Runtime, prelude::*};

use super::field_validation::{
    CredentialField, CredentialFieldError, CredentialFieldErrors, credential_field_issue_message,
    use_credential_fields,
};
use super::labels::{CredentialFeedback, credential_kind_label};
use crate::api::create_admin_credential;
use crate::surface::admin_world_label;
use crate::surface::config::{FieldLiveRegion, use_field_announcer};
use crate::surface::pending::{
    CommandDelivery, CommandReceipt, PendingCommands, advance_command_reload, set_if_alive,
};

// 新凭据尚无服务端 ID，导入入口在本会话内共用一个串行实体。
const IMPORT_ENTITY: &str = "credential:import";

/// 只选择当前导入来源；获取成功仍仅写入所选 kind/world 的一条凭据。
#[derive(Clone, Copy, PartialEq, Eq)]
enum CredentialSource {
    Cookie,
    ForumLogin,
}

/// 录入凭据。已提交的导入由会话持有，离页后仍可用同一命令原样重试。
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
    let pending = use_context::<PendingCommands>();
    let busy = move || pending.is_busy(IMPORT_ENTITY);
    let blocked = move || pending.contains(IMPORT_ENTITY);
    let unknown = move || blocked() && !busy();
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
                    if blocked() {
                        return;
                    }
                    announcer.clear();
                    feedback.set(None);
                    let submitted_kind = kind();
                    let submitted_source = source();
                    let submitted_world = world();
                    let submitted_label = label();
                    let expected_player_name = player_name();
                    let submitted_player_name =
                        (submitted_kind == "mail_cookie" && !expected_player_name.is_empty())
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
                    errors.set(Label, Label.issue(&submitted_label));
                    if let Some(name) = &submitted_player_name {
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
                        announcer.invalid_submit(field_id, credential_field_issue_message(issue), true);
                        return;
                    }
                    let action = t!("admin-credentials-import-action",
                        kind: credential_kind_label(&submitted_kind), label: submitted_label.as_str());
                    // 只把冻结输入交给命令工厂。宿主用同一 ID 运行和重试，不再读取表单。
                    let operation = Rc::new(create_admin_credential(move |command_id| AdminCredentialCreateRequest {
                        command_id,
                        kind: submitted_kind,
                        world: submitted_world,
                        label: submitted_label,
                        player_name: submitted_player_name,
                        secret: raw_secret,
                        login,
                    }));
                    pending.submit_with_receipt(
                        IMPORT_ENTITY.to_owned(),
                        action,
                        move |command_id| {
                            let operation = Rc::clone(&operation);
                            async move {
                                #[cfg(all(test, not(target_arch = "wasm32")))]
                                if let Some(reply) = try_consume_context::<import_tests::ImportReply>() {
                                    return reply.fetch([username, password]).await;
                                }
                                (operation.as_ref())(command_id).await
                            }
                        },
                        move |result, source_alive| {
                            let delivered = source_alive && feedback.try_peek().is_ok();
                            if delivered {
                                // 播报任务仍随表单卸载取消，不能继承会话请求的作用域。
                                Runtime::current().in_scope(feedback.origin_scope(), || {
                                    let saved = CredentialFeedback::Success {
                                        restart_required: result.restart_required,
                                    };
                                    announcer.announce(saved.message());
                                    set_if_alive(feedback, Some(saved));
                                    set_if_alive(kind, "mail_cookie".to_owned());
                                    set_if_alive(world, "main".to_owned());
                                    set_if_alive(label, String::new());
                                    set_if_alive(player_name, String::new());
                                    set_if_alive(secret, String::new());
                                    set_if_alive(username, String::new());
                                    set_if_alive(password, String::new());
                                    set_if_alive(source, CredentialSource::Cookie);
                                    fields.reset(&CredentialField::ALL);
                                    advance_command_reload(reload_generation);
                                });
                            }
                            CommandDelivery::reported(delivered, || {
                                result.restart_required.then(|| {
                                    CommandReceipt::Text(t!("admin-credentials-action-restart"))
                                })
                            })
                        },
                        move |code| {
                            if feedback.try_peek().is_err() {
                                return;
                            }
                            Runtime::current().in_scope(feedback.origin_scope(), || {
                                // 未决命令仍占用实体时，禁用的字段通过常驻播报区反馈。
                                let errors = CredentialFieldErrors::from_server(&code, submitted_source == CredentialSource::Cookie);
                                if let Some((field_id, issue)) = errors.first_invalid() {
                                    fields.show(errors);
                                    announcer.invalid_submit(field_id, credential_field_issue_message(issue), !blocked());
                                } else {
                                    let failure = CredentialFeedback::Error(code);
                                    announcer.announce(failure.message());
                                    set_if_alive(feedback, Some(failure));
                                }
                            });
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
                        disabled: blocked(),
                        value: kind(),
                        onchange: move |event| {
                            if blocked() { return; }
                            kind.set(event.value());
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
                        disabled: blocked(),
                        required: true,
                        aria_invalid: fields.error(Label).is_some().to_string(),
                        aria_describedby: fields.error(Label).is_some().then_some("import-label-error"),
                        value: label(),
                        oninput: move |event| {
                            if blocked() { return; }
                            let value = event.value();
                            fields.edited(Label, &value);
                            label.set(value);
                        },
                        onblur: move |_| {
                            if !blocked() {
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
                        disabled: blocked(),
                        value: world(),
                        onchange: move |event| {
                            if blocked() { return; }
                            world.set(event.value());
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
                        disabled: blocked(),
                        autocomplete: "off",
                        autocapitalize: "none",
                        "autocorrect": "off",
                        spellcheck: "false",
                        aria_invalid: fields.error(PlayerName).is_some().to_string(),
                        aria_describedby: if fields.error(PlayerName).is_some() { "import-player-name-help import-player-name-error" } else { "import-player-name-help" },
                        value: player_name(),
                        oninput: move |event| {
                            if blocked() { return; }
                            let value = event.value();
                            fields.edited(PlayerName, &value);
                            player_name.set(value);
                        },
                        onblur: move |_| {
                            if !blocked() {
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
                        disabled: blocked(),
                        aria_describedby: "import-source-help",
                        value: match source() {
                            CredentialSource::Cookie => "cookie",
                            CredentialSource::ForumLogin => "forum_login",
                        },
                        onchange: move |event| {
                            if blocked() { return; }
                            source.set(if event.value() == "forum_login" {
                                CredentialSource::ForumLogin
                            } else {
                                CredentialSource::Cookie
                            });
                            // 已知未写入时切换来源，清除离开分支的敏感值，避免隐藏表单继续持有。
                            secret.set(String::new());
                            username.set(String::new());
                            password.set(String::new());
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
                        disabled: blocked(),
                        required: true,
                        aria_invalid: fields.error(Secret).is_some().to_string(),
                        aria_describedby: if fields.error(Secret).is_some() { "import-secret-help import-secret-error" } else { "import-secret-help" },
                        value: secret(),
                        oninput: move |event| {
                            if blocked() { return; }
                            let value = event.value();
                            fields.edited(Secret, &value);
                            secret.set(value);
                        },
                        onblur: move |_| {
                            if !blocked() {
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
                        disabled: blocked(),
                        required: true,
                        maxlength: 256,
                        aria_invalid: fields.error(Username).is_some().to_string(),
                        aria_describedby: if fields.error(Username).is_some() { "import-login-help import-username-error" } else { "import-login-help" },
                        value: username(),
                        oninput: move |event| {
                            if blocked() { return; }
                            let value = event.value();
                            fields.edited(Username, &value);
                            username.set(value);
                        },
                        onblur: move |_| {
                            if !blocked() {
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
                        disabled: blocked(),
                        required: true,
                        maxlength: 4096,
                        aria_invalid: fields.error(Password).is_some().to_string(),
                        aria_describedby: if fields.error(Password).is_some() { "import-login-help import-password-error" } else { "import-login-help" },
                        value: password(),
                        oninput: move |event| {
                            if blocked() { return; }
                            let value = event.value();
                            fields.edited(Password, &value);
                            password.set(value);
                        },
                        onblur: move |_| {
                            if !blocked() {
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
                    disabled: blocked(),
                    if busy() { {t!("admin-action-working")} }
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
    use std::assert_matches;
    use std::rc::Rc;

    use auction_web_shared::i18n::{DEFAULT_ADMIN_LOCALE, build_bundle};
    use dioxus::core::{AttributeValue, ElementId, Mutation};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::CredentialCreateForm;
    use crate::surface::ADMIN_LOCALE_CATALOG;
    use crate::surface::config::test_form::{FormBrowser, dispatch, listener};
    use crate::surface::pending::{use_pending_command_host, use_pending_commands};

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
        use_pending_command_host();
        use_pending_commands(reload_generation, |entity| entity.starts_with("credential:"));
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
        assert_matches!(
            attribute(&invalid_cookie, cookie, "aria-invalid"),
            Some(AttributeValue::Text(value)) if value == "true"
        );

        dispatch(&dom, label, "input", " ");
        dispatch(&dom, label, "blur", "");
        let invalid_label = dom.render_immediate_to_vec().edits;
        assert_matches!(
            attribute(&invalid_label, label, "aria-invalid"),
            Some(AttributeValue::Text(value)) if value == "true"
        );
        dispatch(&dom, cookie, "input", "example=value");
        let corrected_cookie = dom.render_immediate_to_vec().edits;
        assert_matches!(
            attribute(&corrected_cookie, cookie, "aria-invalid"),
            Some(AttributeValue::Text(value)) if value == "false"
        );
        browser.fire_timer();
        assert!(dom.render_immediate_to_vec().edits.iter().any(|edit| matches!(edit,
            Mutation::SetText { value, .. } if value == "请填写此字段。"
        )));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(super) mod import_tests {
    use std::cell::Cell;
    use std::cell::RefCell;
    use std::rc::{Rc, Weak};

    use auction_contracts::{AdminCredentialMutationResponse, ApiResponse};
    use auction_web_shared::TransportError;
    use dioxus::core::{AttributeValue, ElementId, Mutation};
    use dioxus::prelude::{
        Element, ReadableExt, Signal, WritableExt, component, dioxus_core, dioxus_elements,
        dioxus_signals, provide_context, rsx, try_consume_context, use_context,
        use_context_provider, use_hook, use_memo, use_signal,
    };
    use gloo_net::http::Method;
    use serde_json::Value;
    use serde_json::json;
    use uuid::Uuid;

    use super::CredentialCreateForm;
    use crate::api::{
        AdminApiFailure, AdminSessionWritePermission, AdminWriteFailure, WriteDispatchStub,
        WriteTransportStub,
    };
    use crate::surface::auctions::editor::tests::{
        RecordRead, assert_field, button, click_button, command_button, has_live_text, has_text,
        init_i18n, mount_writer, settle_writer,
    };
    use crate::surface::pending::{
        PendingCommandHost, PendingCommandNotices, use_pending_command_host, use_pending_commands,
    };
    use crate::surface::realtime::{TestPage, submit_forms, text_attributes};

    pub(in crate::surface::credentials) const PATH: &str = "/api/v1/admin/credentials";
    const LABEL: &str = "测试凭据";
    const COOKIE: &str = "fake-test-cookie=placeholder";
    const USERNAME: &str = "fake-test-forum-account";
    const PASSWORD: &str = "fake-test-forum-password";
    const UNSUPPORTED: &str =
        r#"{"code":"feature_not_supported","message":"测试拒绝","data":null,"request_id":null}"#;
    const LOGIN_UNAVAILABLE: &str = r#"{"code":"credential_login_unavailable","message":"测试拒绝","data":null,"request_id":null}"#;
    const STORAGE_UNAVAILABLE: &str =
        r#"{"code":"storage_unavailable","message":"测试拒绝","data":null,"request_id":null}"#;
    // 导入与同 ID 重放返回 201 / ok，账本保存首次写入时的 active 凭据。
    pub(in crate::surface::credentials) const IMPORT_CREATED: &[u8] = r#"{
        "code":"ok","message":"凭据已导入","data":{
            "credential":{
                "id":"0f8fad5b-d9cb-469f-a165-70867728950e","kind":"mail_cookie",
                "world":"main","label":"测试凭据","player_name":null,"status":"active",
                "state_version":1,"created_at":"2026-11-18T00:00:00Z","rotated_at":null,
                "updated_at":"2026-11-18T00:00:00Z"
            },
            "restart_required":true
        }
    }"#
    .as_bytes();

    #[component]
    fn ImportSource() -> Element {
        let reload_generation = use_signal(|| 0);
        use_pending_commands(reload_generation, |entity| entity.starts_with("credential:"));
        rsx! { CredentialCreateForm { reload_generation } }
    }

    fn mount(
        status: u16,
        body: &'static str,
        retry: (u16, &'static str),
    ) -> (TestPage, Vec<Mutation>) {
        mount_writer(move || {
            init_i18n();
            use_hook(move || {
                AdminApiFailure::stub_write_response(PATH, Method::POST, status, body);
            });
            let mut visible = use_signal(|| true);
            let mut allowed = use_signal(|| true);
            let permission = use_memo(move || *allowed.read());
            provide_context(AdminSessionWritePermission(permission));
            let host = use_context::<PendingCommandHost>();
            rsx! {
                button { onclick: move |_| visible.toggle(), {"切换凭据页".to_owned()} }
                button { onclick: move |_| allowed.set(false), {"暂停写许可".to_owned()} }
                button { onclick: move |_| AdminApiFailure::stub_write_response(PATH, Method::POST, retry.0, retry.1),
                    {"切换测试回执".to_owned()}
                }
                PendingCommandNotices { host }
                if visible() { ImportSource {} }
            }
        })
    }

    fn listeners<'a>(edits: &'a [Mutation], name: &'a str) -> impl Iterator<Item = ElementId> + 'a {
        edits.iter().filter_map(move |edit| match edit {
            Mutation::NewEventListener { name: event, id } if *event == name => Some(*id),
            _ => None,
        })
    }

    fn label(mounted: &[Mutation]) -> ElementId {
        listeners(mounted, "input").next().expect("缺少标签输入")
    }

    pub(in crate::surface::credentials) fn submit_import(
        view: &mut TestPage,
        mounted: &[Mutation],
        forum_login: bool,
    ) -> Vec<Mutation> {
        let submit = submit_forms(mounted).next().unwrap();
        view.dispatch("input", label(mounted), LABEL);
        if forum_login {
            let source = listeners(mounted, "change").nth(2).expect("缺少导入来源");
            let login = view.dispatch("change", source, "forum_login");
            let mut inputs = listeners(&login, "input");
            view.dispatch("input", inputs.next().expect("缺少论坛账号输入"), USERNAME);
            view.dispatch("input", inputs.next().expect("缺少论坛密码输入"), PASSWORD);
            assert!(inputs.next().is_none());
        } else {
            let (cookie, _) = text_attributes(mounted, "id")
                .find(|(_, id)| *id == "import-secret")
                .expect("缺少 Cookie 输入");
            view.dispatch("input", cookie, COOKIE);
        }
        view.dispatch("submit", submit, "")
    }

    const RESTART: &str = "操作已完成。请重启 Worker 以应用凭据变更。";

    #[derive(Clone)]
    pub(super) struct ImportReply(
        Rc<RecordRead<AdminCredentialMutationResponse, AdminWriteFailure>>,
    );

    #[derive(Clone, Default)]
    struct ForumDraftProbe(Rc<Cell<Option<[Signal<String>; 2]>>>);

    impl ImportReply {
        pub(super) async fn fetch(
            &self,
            fields: [Signal<String>; 2],
        ) -> Result<AdminCredentialMutationResponse, AdminWriteFailure> {
            if let Some(probe) = try_consume_context::<ForumDraftProbe>() {
                probe.0.set(Some(fields));
            }
            self.0.fetch().await
        }
    }

    #[component]
    fn ControlledSession() -> Element {
        let host = use_pending_command_host();
        let mut visible = use_signal(|| true);
        rsx! {
            button { onclick: move |_| visible.toggle(), {"切换凭据页".to_owned()} }
            PendingCommandNotices { host }
            if visible() { ImportSource {} }
        }
    }

    fn mount_controlled(reply: ImportReply) -> (TestPage, Vec<Mutation>) {
        mount_writer(move || {
            init_i18n();
            use_context_provider(|| reply.clone());
            let mut authenticated = use_signal(|| true);
            rsx! {
                button { onclick: move |_| authenticated.toggle(), {"切换认证宿主".to_owned()} }
                if authenticated() { ControlledSession {} }
            }
        })
    }

    fn assert_safe_output(edits: &[Mutation]) {
        for secret in [COOKIE, USERNAME, PASSWORD] {
            assert!(!has_text(edits, secret));
            assert!(!edits.iter().any(|edit| matches!(edit,
                Mutation::SetAttribute { name, value: AttributeValue::Text(value), .. }
                    if *name != "value" && value.contains(secret)
            )));
        }
    }

    fn assert_empty_secret(edits: &[Mutation], target: ElementId) {
        assert!(
            text_attributes(edits, "value").any(|(id, value)| id == target && value.is_empty())
        );
    }

    fn cookie_field(edits: &[Mutation]) -> ElementId {
        text_attributes(edits, "id").find(|(_, value)| *value == "import-secret").unwrap().0
    }

    struct ImportDispatch {
        url: String,
        method: Method,
        body: Vec<u8>,
        timeout_millis: u32,
    }

    fn assert_import_dispatch_and_shell_retry(forum_login: bool) {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let observed = requests.clone();
        let (mut view, mounted) = mount_writer(move || {
            init_i18n();
            let observed = observed.clone();
            use_hook(move || {
                WriteTransportStub::install(
                    || Ok("csrf-fixture".to_owned()),
                    move |url, method, body, timeout_millis| {
                        let mut requests = observed.borrow_mut();
                        requests.push(ImportDispatch {
                            url: url.to_owned(),
                            method: method.clone(),
                            body: body.to_vec(),
                            timeout_millis,
                        });
                        match requests.len() {
                            1 => WriteDispatchStub::Failed {
                                built: true,
                                error: TransportError::Network(None),
                            },
                            2 => WriteDispatchStub::Response {
                                status: 201,
                                body: Some(IMPORT_CREATED),
                            },
                            _ => panic!("出现额外的导入派发"),
                        }
                    },
                );
            });
            rsx! { ControlledSession {} }
        });

        let submitted = submit_import(&mut view, &mounted, forum_login);
        let command_id: Uuid = text_attributes(&submitted, "data-command-id")
            .next()
            .expect("缺少未决导入记录")
            .1
            .parse()
            .unwrap();
        {
            let requests = requests.borrow();
            assert_eq!(requests.len(), 1);
            let first = &requests[0];
            assert_eq!(first.url, PATH);
            assert_eq!(first.method, Method::POST);
            assert_eq!(first.timeout_millis, 20_000, "首次导入预算");
            let mut expected = json!({
                "command_id": command_id, "kind": "mail_cookie", "world": "main", "label": LABEL,
            });
            if forum_login {
                expected["login"] = json!({"username": USERNAME, "password": PASSWORD});
            } else {
                expected["secret"] = json!(COOKIE);
            }
            let body: Value = serde_json::from_slice(&first.body).unwrap();
            assert!(body == expected, "首次导入正文与表单及面板上的命令 ID 不一致");
        }

        click_button(&mut view, "切换凭据页");
        assert_eq!(requests.borrow().len(), 1);
        let (retry, disabled) =
            command_button(&view, command_id, "原样重试").expect("离页后应保留原命令");
        assert!(!disabled);
        view.dispatch("click", retry, "");

        let requests = requests.borrow();
        assert_eq!(requests.len(), 2);
        let (first, second) = (&requests[0], &requests[1]);
        assert_eq!(second.url, first.url);
        assert_eq!(second.method, first.method);
        assert_eq!(second.timeout_millis, 20_000, "导入重试预算");
        assert!(second.body == first.body, "重试应逐字节复用首次派发的正文");
        assert!(button(&view, "原样重试").is_none());
        assert!(button(&view, "关闭回执").is_some());
        assert!(has_live_text(&view, RESTART));
    }

    #[test]
    fn cookie_import_dispatches_identical_requests_on_shell_retry() {
        assert_import_dispatch_and_shell_retry(false);
    }

    #[test]
    fn forum_login_import_dispatches_identical_requests_on_shell_retry() {
        assert_import_dispatch_and_shell_retry(true);
    }

    fn assert_unusable_import_receipt_keeps_the_original_command(invalid_body: Vec<u8>) {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let (mut view, mounted) = mount_writer({
            let observed = requests.clone();
            move || {
                init_i18n();
                let observed = observed.clone();
                let invalid_body = invalid_body.clone();
                use_hook(move || {
                    WriteTransportStub::install(
                        || Ok("csrf-fixture".to_owned()),
                        move |url, method, body, timeout_millis| {
                            assert_eq!(url, PATH);
                            assert_eq!(*method, Method::POST);
                            assert_eq!(timeout_millis, 20_000);
                            let mut requests = observed.borrow_mut();
                            requests.push(body.to_vec());
                            let body = match requests.len() {
                                1 | 2 => invalid_body.clone(),
                                3 => IMPORT_CREATED.to_vec(),
                                _ => panic!("出现额外的导入派发"),
                            };
                            WriteDispatchStub::OwnedResponse { status: 201, body }
                        },
                    );
                });
                rsx! { ControlledSession {} }
            }
        });

        let submitted = submit_import(&mut view, &mounted, false);
        let command_id: Uuid = text_attributes(&submitted, "data-command-id")
            .next()
            .expect("缺少未决导入记录")
            .1
            .parse()
            .unwrap();
        let pending_retry = |view: &TestPage| {
            assert!(!has_live_text(view, RESTART));
            assert!(button(view, "关闭回执").is_none());
            let (retry, disabled) =
                command_button(view, command_id, "原样重试").expect("异常回执应保留原命令");
            assert!(!disabled);
            retry
        };
        pending_retry(&view);
        let label_input = label(&mounted);
        let disabled = submitted.iter().rev().find_map(|edit| match edit {
            Mutation::SetAttribute { id, name: "disabled", value, .. } if *id == label_input => {
                Some(value)
            }
            _ => None,
        });
        assert_eq!(disabled, Some(&AttributeValue::Bool(true)), "未决导入应锁住表单");
        let submit = submit_forms(&mounted).next().unwrap();
        view.dispatch("submit", submit, "");
        assert_eq!(requests.borrow().len(), 1, "原命令未决时禁止重新提交");

        click_button(&mut view, "切换凭据页");
        let retry = pending_retry(&view);
        view.dispatch("click", retry, "");
        {
            let requests = requests.borrow();
            assert_eq!(requests.len(), 2);
            assert!(requests[1] == requests[0], "异常回执后的重试应复用原始正文");
        }
        let retry = pending_retry(&view);
        view.dispatch("click", retry, "");
        let requests = requests.borrow();
        assert_eq!(requests.len(), 3);
        assert!(requests[2] == requests[0], "了结时的重试应复用原始正文");
        assert!(command_button(&view, command_id, "原样重试").is_none());
        assert!(button(&view, "关闭回执").is_some());
        assert!(has_live_text(&view, RESTART));
    }

    #[test]
    fn import_with_mismatched_receipt_label_keeps_the_original_command() {
        let mut payload: ApiResponse<AdminCredentialMutationResponse> =
            serde_json::from_slice(IMPORT_CREATED).unwrap();
        payload.data.credential.label = "另一条测试凭据".to_owned();
        assert_unusable_import_receipt_keeps_the_original_command(
            serde_json::to_vec(&payload).unwrap(),
        );
    }

    #[test]
    fn import_with_malformed_success_body_keeps_the_original_command() {
        assert_unusable_import_receipt_keeps_the_original_command(
            br#"{"code":"ok","data":"#.to_vec(),
        );
    }

    #[test]
    fn import_first_success_after_leaving_is_delivered_to_the_session() {
        let reply = Rc::new(RecordRead::new());
        let (mut view, mounted) = mount_controlled(ImportReply(reply.clone()));
        let submitted = submit_import(&mut view, &mounted, false);
        assert_safe_output(&submitted);
        assert_eq!(reply.started(), 1);
        assert!(!has_live_text(&view, RESTART));
        click_button(&mut view, "切换凭据页");
        reply.respond(Ok(AdminCredentialMutationResponse {
            credential: crate::api::AdminCredential {
                label: LABEL.to_owned(),
                status: "disabled".to_owned(),
                ..super::super::table::tests::credential()
            },
            restart_required: true,
        }));
        let edits = settle_writer(&mut view);
        assert_safe_output(&edits);
        assert!(has_live_text(&view, RESTART));
        assert!(button(&view, "关闭回执").is_some());
        assert!(button(&view, "原样重试").is_none());
        let mounted_again = click_button(&mut view, "切换凭据页");
        assert_empty_secret(&mounted_again, cookie_field(&mounted_again));
        assert_eq!(reply.started(), 1);
    }

    #[test]
    fn import_success_clears_the_mounted_draft() {
        for forum in [false, true] {
            let reply = Rc::new(RecordRead::new());
            let (mut view, mounted) = mount_controlled(ImportReply(reply.clone()));
            let probe = ForumDraftProbe::default();
            if forum {
                view.dom().provide_root_context(probe.clone());
            }
            let source = listeners(&mounted, "change").nth(2).expect("缺少导入来源");
            submit_import(&mut view, &mounted, forum);
            assert_eq!(reply.started(), 1);
            let forum_draft = forum.then(|| {
                let [username, password] = probe.0.take().expect("缺少挂载表单的论坛登录草稿");
                assert!(username.peek().as_str() == USERNAME, "回包前应保留论坛账号");
                assert!(password.peek().as_str() == PASSWORD, "回包前应保留论坛密码");
                [username, password]
            });

            reply.respond(Ok(AdminCredentialMutationResponse {
                credential: crate::api::AdminCredential {
                    label: LABEL.to_owned(),
                    status: "disabled".to_owned(),
                    ..super::super::table::tests::credential()
                },
                restart_required: true,
            }));
            let saved = settle_writer(&mut view);
            assert!(has_live_text(&view, RESTART));
            assert_field(&view, label(&mounted), "", false);
            let cookie = cookie_field(if forum { &saved } else { &mounted });
            assert_empty_secret(&saved, cookie);
            assert!(button(&view, "原样重试").is_none());
            if let Some([username, password]) = forum_draft {
                // 来源切换也会清空草稿，先检查成功回执处理后的隐藏字段。
                assert!(
                    username.try_peek().expect("导入表单应仍挂载").is_empty(),
                    "成功回执应清空论坛账号"
                );
                assert!(
                    password.try_peek().expect("导入表单应仍挂载").is_empty(),
                    "成功回执应清空论坛密码"
                );
                let login = view.dispatch("change", source, "forum_login");
                let mut inputs = listeners(&login, "input");
                assert_empty_secret(&login, inputs.next().expect("缺少论坛账号输入"));
                assert_empty_secret(&login, inputs.next().expect("缺少论坛密码输入"));
                assert!(inputs.next().is_none());
            }
            assert_eq!(reply.started(), 1);
        }
    }

    #[component]
    fn FrozenImportSession() -> Element {
        let frozen = use_context::<Weak<String>>();
        let host = use_pending_command_host();
        let reload = use_signal(|| 0);
        let owner = use_pending_commands(reload, |entity| entity == super::IMPORT_ENTITY);
        rsx! {
            PendingCommandNotices { host }
            button { onclick: move |_| {
                let frozen = frozen.upgrade().expect("缺少测试冻结正文");
                assert!(owner.submit(
                    super::IMPORT_ENTITY.to_owned(), "测试导入".to_owned(),
                    move |_| {
                        let payload = Rc::clone(&frozen);
                        async move {
                            drop(payload);
                            Err::<(), _>(AdminWriteFailure::outcome_unknown("invalid_response"))
                        }
                    },
                    |()| {}, |_| {},
                ));
            }, {"提交测试导入".to_owned()} }
        }
    }

    #[test]
    fn import_session_end_releases_the_frozen_command() {
        let frozen = Rc::new(COOKIE.to_owned());
        let retained = Rc::downgrade(&frozen);
        let probe = retained.clone();
        let (mut view, _) = mount_writer(move || {
            init_i18n();
            use_context_provider(|| probe.clone());
            let mut authenticated = use_signal(|| true);
            rsx! {
                button { onclick: move |_| authenticated.toggle(), {"切换认证宿主".to_owned()} }
                if authenticated() { FrozenImportSession {} }
            }
        });
        click_button(&mut view, "提交测试导入");
        assert!(button(&view, "原样重试").is_some());
        drop(frozen);
        // 外部只留弱引用，观察宿主是否仍保有结果未知的工厂及其正文。
        assert!(retained.upgrade().is_some());
        click_button(&mut view, "切换认证宿主");
        assert!(retained.upgrade().is_none());
    }

    #[test]
    fn import_draft_and_retry_disappear_after_abandonment_or_session_end() {
        for forum in [false, true] {
            for end_session in [false, true] {
                let reply = Rc::new(RecordRead::new());
                let (mut view, mounted) = mount_controlled(ImportReply(reply.clone()));
                assert_safe_output(&submit_import(&mut view, &mounted, forum));
                assert_safe_output(&click_button(&mut view, "切换凭据页"));
                reply.respond(Err(AdminWriteFailure::outcome_unknown("invalid_response")));
                assert_safe_output(&settle_writer(&mut view));
                assert!(button(&view, "原样重试").is_some());

                if end_session {
                    click_button(&mut view, "切换认证宿主");
                    assert!(button(&view, "原样重试").is_none());
                } else {
                    click_button(&mut view, "放弃");
                    assert!(button(&view, "原样重试").is_some());
                    assert_safe_output(&click_button(&mut view, "确认放弃"));
                }
                assert!(button(&view, "原样重试").is_none());
                let mounted_again = click_button(
                    &mut view,
                    if end_session { "切换认证宿主" } else { "切换凭据页" },
                );
                assert_safe_output(&mounted_again);
                assert_empty_secret(&mounted_again, cookie_field(&mounted_again));
                let source = listeners(&mounted_again, "change").nth(2).unwrap();
                let login = view.dispatch("change", source, "forum_login");
                let mut secrets = listeners(&login, "input");
                assert_empty_secret(&login, secrets.next().expect("缺少论坛账号输入"));
                assert_empty_secret(&login, secrets.next().expect("缺少论坛密码输入"));
                assert!(secrets.next().is_none());
                assert!(button(&view, "原样重试").is_none());
                assert_eq!(reply.started(), 1);
            }
        }
    }

    #[test]
    fn import_retries_the_same_command_after_leaving_for_either_source() {
        for (forum_login, status, receipt) in
            [(false, 501, UNSUPPORTED), (true, 503, LOGIN_UNAVAILABLE)]
        {
            let (mut view, mounted) = mount(503, STORAGE_UNAVAILABLE, (status, receipt));
            let submitted = submit_import(&mut view, &mounted, forum_login);
            let command_id = text_attributes(&submitted, "data-command-id").next().unwrap().1;
            assert_eq!(command_id.parse::<uuid::Uuid>().unwrap().get_version_num(), 4);
            assert!(has_text(&submitted, "导入邮件 Cookie（测试凭据）"));
            for secret in [COOKIE, USERNAME, PASSWORD] {
                assert!(!has_text(&submitted, secret));
            }
            let submit = submit_forms(&mounted).next().unwrap();
            assert!(
                text_attributes(&view.dispatch("submit", submit, ""), "data-command-id")
                    .next()
                    .is_none()
            );
            click_button(&mut view, "切换凭据页");
            click_button(&mut view, "切换测试回执");
            let retried = click_button(&mut view, "原样重试");
            assert!(has_text(
                &retried,
                if forum_login {
                    "论坛登录暂不可用"
                } else {
                    "当前部署未提供此操作"
                }
            ));
            assert!(button(&view, "原样重试").is_some());
            let remounted = click_button(&mut view, "切换凭据页");
            assert!(has_text(&remounted, "请在未决操作区原样重试或放弃"));
            assert!(command_button(&view, command_id.parse().unwrap(), "原样重试").is_some());
        }
    }

    #[test]
    fn import_rejection_releases_the_first_attempt_but_plain_503_can_be_abandoned_while_paused() {
        for (forum_login, status, receipt) in
            [(false, 501, UNSUPPORTED), (true, 503, LOGIN_UNAVAILABLE)]
        {
            let (mut view, mounted) = mount(status, receipt, (503, STORAGE_UNAVAILABLE));
            submit_import(&mut view, &mounted, forum_login);
            assert!(button(&view, "原样重试").is_none());
            assert_field(&view, label(&mounted), LABEL, false);
        }

        let (mut view, mounted) = mount(503, STORAGE_UNAVAILABLE, (501, UNSUPPORTED));
        let submitted = submit_import(&mut view, &mounted, false);
        assert!(has_text(&submitted, "请在未决操作区原样重试或放弃"));
        assert!(button(&view, "原样重试").is_some());
        click_button(&mut view, "放弃");
        click_button(&mut view, "保留未决操作");
        assert!(button(&view, "原样重试").is_some());
        click_button(&mut view, "暂停写许可");
        assert!(button(&view, "原样重试").is_some_and(|(_, disabled)| disabled));
        click_button(&mut view, "放弃");
        click_button(&mut view, "确认放弃");
        assert!(button(&view, "原样重试").is_none());
        assert_field(&view, label(&mounted), LABEL, false);
    }
}
