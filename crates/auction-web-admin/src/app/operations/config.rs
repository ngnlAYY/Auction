//! Admin runtime 配置页面。
//!
//! 页面只编辑服务端下发的非敏感 JSON 投影；候选必须先经过服务端规范化校验，更新再以
//! fingerprint 和可重试的 command ID 作为版本与幂等边界。TOML、密钥和命令账本永远不进入浏览器。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use serde_json::Value;
use uuid::Uuid;

use crate::api::{
    fetch_admin_runtime_config, update_admin_runtime_config, validate_admin_runtime_config,
};

use super::admin_api_error_message;

#[derive(Debug, Clone)]
struct ConfigCommand {
    command_id: Uuid,
    fingerprint: String,
    document: Value,
    signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfigNotice {
    Validated,
    Saved,
    InvalidJson,
    ActionError(String),
    Conflict,
}

/// 加载并展示 Admin runtime 非敏感配置。
pub(super) fn page() -> Element {
    let mut editor = use_signal(|| None::<String>);
    let mut fingerprint = use_signal(|| None::<String>);
    let mut checked_signature = use_signal(String::new);
    let mut command = use_signal(|| None::<ConfigCommand>);
    let mut notice = use_signal(|| None::<ConfigNotice>);
    let mut restart_required = use_signal(|| false);
    let mut refresh = use_signal(|| 0_u64);
    let busy = use_signal(|| false);
    let config = use_resource(move || {
        let _ = refresh();
        fetch_admin_runtime_config()
    });

    // 只有服务端快照变化时才初始化文本框，不能在普通输入重绘时覆盖用户草稿。
    use_effect(move || {
        let snapshot = config.read_unchecked().clone();
        if let Some(Ok(value)) = snapshot {
            editor.set(Some(format_document(&value.document)));
            fingerprint.set(Some(value.fingerprint));
            restart_required.set(value.restart_required);
            checked_signature.set(String::new());
            command.set(None);
            notice.set(None);
        }
    });

    let content = match &*config.read_unchecked() {
        None => rsx! {
            p { class: "admin-config-status", role: "status", {t!("admin-runtime-loading")} }
        },
        Some(Err(error)) => rsx! {
            div { class: "admin-config-error", role: "alert",
                p { {admin_api_error_message(&error.code)} }
                button {
                    class: "admin-btn-secondary",
                    r#type: "button",
                    onclick: move |_| refresh.set(refresh().saturating_add(1)),
                    {t!("admin-retry")}
                }
            }
        },
        Some(Ok(snapshot)) => {
            let editor_value = editor().unwrap_or_else(|| format_document(&snapshot.document));
            let current_fingerprint = fingerprint().unwrap_or_else(|| snapshot.fingerprint.clone());
            let can_update =
                !busy() && !editor_value.is_empty() && checked_signature() == editor_value;
            let validate_text = editor_value.clone();
            let update_text = editor_value.clone();
            let update_fingerprint = current_fingerprint.clone();
            let (enabled_worlds, equipment_filter_enabled) =
                runtime_business_facts(&snapshot.document);

            rsx! {
                div {
                    class: "admin-runtime-config__content",
                    aria_busy: busy().to_string(),
                    div { class: "admin-config-facts",
                        AdminRuntimeConfigFact {
                            detail: format!("{enabled_worlds}/2"),
                            enabled: enabled_worlds > 0,
                            label: t!("admin-runtime-enabled-worlds"),
                        }
                        AdminRuntimeConfigFact {
                            detail: current_fingerprint.clone(),
                            enabled: equipment_filter_enabled,
                            label: t!("admin-runtime-equipment-filter"),
                        }
                    }
                    form {
                        class: "admin-runtime-config__form admin-config-form",
                        onsubmit: {
                            let text = validate_text.clone();
                            move |event| {
                                event.prevent_default();
                                notice.set(None);
                                let document = match parse_document(&text) {
                                    Ok(document) => document,
                                    Err(_) => {
                                        notice.set(Some(ConfigNotice::InvalidJson));
                                        return;
                                    }
                                };
                                let mut busy = busy;
                                let mut editor = editor;
                                let mut checked_signature = checked_signature;
                                let mut command = command;
                                let mut notice = notice;
                                let mut restart_required = restart_required;
                                busy.set(true);
                                spawn(async move {
                                    match validate_admin_runtime_config(document).await {
                                        Ok(result) => {
                                            let normalized = format_document(&result.document);
                                            editor.set(Some(normalized.clone()));
                                            checked_signature.set(normalized);
                                            restart_required.set(result.restart_required);
                                            command.set(None);
                                            notice.set(Some(ConfigNotice::Validated));
                                        }
                                        Err(error) => {
                                            notice.set(Some(if error.is_conflict() {
                                                ConfigNotice::Conflict
                                            } else {
                                                ConfigNotice::ActionError(error.code)
                                            }));
                                        }
                                    }
                                    busy.set(false);
                                });
                            }
                        },
                        label { class: "admin-runtime-config__label admin-config-form__label",
                            for: "admin-runtime-document",
                            {t!("admin-runtime-editor")}
                            textarea {
                                id: "admin-runtime-document",
                                class: "admin-runtime-config__editor",
                                disabled: busy(),
                                required: true,
                                spellcheck: "false",
                                value: editor_value.clone(),
                                oninput: move |event| {
                                    editor.set(Some(event.value()));
                                    checked_signature.set(String::new());
                                    command.set(None);
                                    notice.set(None);
                                },
                            }
                        }
                        if let Some(current_notice) = notice() {
                            p {
                                class: if matches!(
                                    &current_notice,
                                    ConfigNotice::Validated | ConfigNotice::Saved
                                ) {
                                    "admin-panel__note action-feedback action-feedback--success"
                                } else {
                                    "admin-panel__note action-feedback action-feedback--error"
                                },
                                role: if matches!(
                                    &current_notice,
                                    ConfigNotice::Validated | ConfigNotice::Saved
                                ) { "status" } else { "alert" },
                                {notice_text(&current_notice)}
                            }
                        }
                        if restart_required() || snapshot.restart_required {
                            p { class: "admin-panel__note admin-runtime-config__restart", role: "status", {t!("admin-runtime-restart-required")} }
                        }
                        div { class: "admin-panel__actions",
                            button {
                                class: "admin-btn-secondary",
                                r#type: "button",
                                disabled: busy(),
                                onclick: move |_| {
                                    editor.set(None);
                                    fingerprint.set(None);
                                    checked_signature.set(String::new());
                                    command.set(None);
                                    notice.set(None);
                                    refresh.set(refresh().saturating_add(1));
                                },
                                {t!("admin-runtime-reload")}
                            }
                            button {
                                class: "admin-btn-secondary",
                                r#type: "submit",
                                disabled: busy(),
                                if busy() { {t!("admin-action-working")} } else { {t!("admin-runtime-validate")} }
                            }
                            button {
                                class: "admin-btn-primary",
                                r#type: "button",
                                disabled: !can_update,
                                onclick: {
                                    let text = update_text.clone();
                                    let expected_fingerprint = update_fingerprint.clone();
                                    move |_| {
                                        let document = match parse_document(&text) {
                                            Ok(document) => document,
                                            Err(_) => {
                                                notice.set(Some(ConfigNotice::InvalidJson));
                                                return;
                                            }
                                        };
                                        let compact = serde_json::to_string(&document).unwrap_or_default();
                                        let command_key = format!("{}:{compact}", expected_fingerprint);
                                        let current_command = match command() {
                                            Some(existing) if existing.signature == command_key => existing,
                                            _ => ConfigCommand {
                                                command_id: Uuid::new_v4(),
                                                fingerprint: expected_fingerprint.clone(),
                                                document: document.clone(),
                                                signature: command_key,
                                            },
                                        };
                                        command.set(Some(current_command.clone()));
                                        let mut busy = busy;
                                        let mut notice = notice;
                                        let mut editor = editor;
                                        let mut checked_signature = checked_signature;
                                        let mut fingerprint = fingerprint;
                                        let mut restart_required = restart_required;
                                        busy.set(true);
                                        notice.set(None);
                                        spawn(async move {
                                            match update_admin_runtime_config(
                                                current_command.command_id,
                                                current_command.fingerprint,
                                                current_command.document,
                                            )
                                            .await
                                            {
                                                Ok(result) => {
                                                    let normalized = format_document(&result.document);
                                                    editor.set(Some(normalized));
                                                    checked_signature.set(String::new());
                                                    fingerprint.set(Some(result.fingerprint));
                                                    restart_required.set(result.restart_required);
                                                    notice.set(Some(ConfigNotice::Saved));
                                                }
                                                Err(error) => {
                                                    notice.set(Some(if error.is_conflict() {
                                                        ConfigNotice::Conflict
                                                    } else {
                                                        ConfigNotice::ActionError(error.code)
                                                    }));
                                                }
                                            }
                                            busy.set(false);
                                        });
                                    }
                                },
                                if busy() { {t!("admin-action-working")} } else { {t!("admin-runtime-update")} }
                            }
                        }
                    }
                }
            }
        }
    };

    rsx! {
        section {
            class: "admin-config-page",
            aria_labelledby: "config-title",
            h1 { id: "config-title", {t!("page-config-title")} }
            p { class: "admin-config-intro", {t!("admin-runtime-note")} }
            {content}
        }
    }
}

#[component]
fn AdminRuntimeConfigFact(detail: String, enabled: bool, label: String) -> Element {
    rsx! {
        article {
            div { class: "admin-config-fact-heading",
                strong { "{label}" }
                span {
                    class: if enabled {
                        "admin-config-fact-enabled"
                    } else {
                        "admin-config-fact-disabled"
                    },
                    if enabled { {t!("admin-runtime-enabled")} } else { {t!("admin-runtime-disabled")} }
                }
            }
            p { "{detail}" }
        }
    }
}

fn notice_text(notice: &ConfigNotice) -> String {
    match notice {
        ConfigNotice::Validated => t!("admin-runtime-validated"),
        ConfigNotice::Saved => t!("admin-runtime-action-success"),
        ConfigNotice::InvalidJson => t!("admin-runtime-invalid-json"),
        ConfigNotice::ActionError(code) => admin_api_error_message(code),
        ConfigNotice::Conflict => t!("admin-runtime-conflict"),
    }
}

fn parse_document(text: &str) -> Result<Value, ()> {
    let document = serde_json::from_str::<Value>(text).map_err(|_| ())?;
    if document.is_object() { Ok(document) } else { Err(()) }
}

fn format_document(document: &Value) -> String {
    serde_json::to_string_pretty(document).unwrap_or_else(|_| "{}".to_owned())
}

fn runtime_business_facts(document: &Value) -> (usize, bool) {
    let enabled_worlds = ["main", "isekai"]
        .into_iter()
        .filter(|world| {
            document
                .get("worlds")
                .and_then(|worlds| worlds.get(*world))
                .and_then(|config| config.get("enabled"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .count();
    let equipment_filter_enabled = document
        .get("equipment_filter")
        .and_then(|config| config.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    (enabled_worlds, equipment_filter_enabled)
}
