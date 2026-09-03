//! Admin runtime 配置页面。
//!
//! 页面用类型化表单编辑服务端下发的完整非敏感投影；候选必须先经过服务端规范化校验，
//! 更新再以 fingerprint 和可重试的 command ID 作为版本与幂等边界。成功更新会由服务端
//! 原子写入 runtime TOML，角色启动配置、公共配置、密钥和命令账本永远不进入浏览器。

#[path = "config/form.rs"]
mod form;
#[path = "config/model.rs"]
mod model;

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use serde_json::Value;
use uuid::Uuid;

use crate::api::{
    fetch_admin_runtime_config, update_admin_runtime_config, validate_admin_runtime_config,
};

use self::{form::RuntimeConfigFields, model::RuntimeConfigForm};
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
    InvalidForm,
    ActionError(String),
    Conflict,
}

/// 加载并展示 Admin runtime 非敏感配置。
pub(super) fn page() -> Element {
    let mut form = use_signal(RuntimeConfigForm::default);
    let mut form_ready = use_signal(|| false);
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

    // 只有服务端快照变化时才初始化表单，不能在普通输入重绘时覆盖用户草稿。
    use_effect(move || {
        let snapshot = config.read_unchecked().clone();
        if let Some(Ok(value)) = snapshot {
            match RuntimeConfigForm::from_document(&value.document) {
                Ok(next_form) => {
                    form.set(next_form);
                    form_ready.set(true);
                    fingerprint.set(Some(value.fingerprint));
                    restart_required.set(value.restart_required);
                    checked_signature.set(String::new());
                    command.set(None);
                    notice.set(None);
                }
                Err(()) => {
                    form_ready.set(false);
                    notice.set(Some(ConfigNotice::InvalidForm));
                }
            }
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
        Some(Ok(_)) if !form_ready() => {
            if matches!(notice(), Some(ConfigNotice::InvalidForm)) {
                rsx! {
                    div { class: "admin-config-error", role: "alert",
                        p { {t!("admin-runtime-invalid-form")} }
                        button {
                            class: "admin-btn-secondary",
                            r#type: "button",
                            onclick: move |_| refresh.set(refresh().saturating_add(1)),
                            {t!("admin-retry")}
                        }
                    }
                }
            } else {
                rsx! {
                    p { class: "admin-config-status", role: "status", {t!("admin-runtime-loading")} }
                }
            }
        }
        Some(Ok(snapshot)) => {
            let current_fingerprint = fingerprint().unwrap_or_else(|| snapshot.fingerprint.clone());
            let candidate_signature = form.read().signature().ok();
            let can_update = !busy()
                && candidate_signature
                    .as_deref()
                    .is_some_and(|signature| signature == checked_signature());
            let enabled_worlds = ["main", "isekai"]
                .into_iter()
                .filter(|world| form.read().boolean(&format!("worlds.{world}.enabled")))
                .count();
            let equipment_filter_enabled = form.read().boolean("equipment_filter.enabled");
            let update_fingerprint = current_fingerprint.clone();

            rsx! {
                div {
                    class: "admin-runtime-config__content",
                    aria_busy: busy().to_string(),
                    div { class: "admin-config-facts",
                        AdminRuntimeConfigFact {
                            detail: format!("{enabled_worlds}/2"),
                            state: Some(enabled_worlds > 0),
                            label: t!("admin-runtime-enabled-worlds"),
                        }
                        AdminRuntimeConfigFact {
                            detail: if equipment_filter_enabled {
                                t!("admin-runtime-enabled")
                            } else {
                                t!("admin-runtime-disabled")
                            },
                            state: Some(equipment_filter_enabled),
                            label: t!("admin-runtime-equipment-filter"),
                        }
                        AdminRuntimeConfigFact {
                            detail: current_fingerprint.clone(),
                            state: None,
                            label: t!("admin-runtime-fingerprint-label"),
                        }
                    }
                    form {
                        class: "admin-runtime-config__form admin-config-form",
                        onsubmit: {
                            let form = form;
                            move |event| {
                                event.prevent_default();
                                notice.set(None);
                                let document = match form.read().to_document() {
                                    Ok(document) => document,
                                    Err(()) => {
                                        notice.set(Some(ConfigNotice::InvalidForm));
                                        return;
                                    }
                                };
                                let mut busy = busy;
                                let mut form = form;
                                let mut checked_signature = checked_signature;
                                let mut command = command;
                                let mut notice = notice;
                                busy.set(true);
                                spawn(async move {
                                    match validate_admin_runtime_config(document).await {
                                        Ok(result) => match RuntimeConfigForm::from_document(
                                            &result.document,
                                        ) {
                                            Ok(next_form) => match next_form.signature() {
                                                Ok(signature) => {
                                                    form.set(next_form);
                                                    checked_signature.set(signature);
                                                    command.set(None);
                                                    notice.set(Some(ConfigNotice::Validated));
                                                }
                                                Err(()) => notice
                                                    .set(Some(ConfigNotice::InvalidForm)),
                                            },
                                            Err(()) => {
                                                notice.set(Some(ConfigNotice::InvalidForm));
                                            }
                                        },
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
                        RuntimeConfigFields {
                            form,
                            disabled: busy(),
                            on_dirty: move |_| {
                                checked_signature.set(String::new());
                                command.set(None);
                                notice.set(None);
                            },
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
                            p {
                                class: "admin-panel__note admin-runtime-config__restart",
                                role: "status",
                                {t!("admin-runtime-restart-required")}
                            }
                        }
                        div { class: "admin-panel__actions",
                            button {
                                class: "admin-btn-secondary",
                                r#type: "button",
                                disabled: busy(),
                                onclick: move |_| {
                                    form_ready.set(false);
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
                                if busy() {
                                    {t!("admin-action-working")}
                                } else {
                                    {t!("admin-runtime-validate")}
                                }
                            }
                            button {
                                class: "admin-btn-primary",
                                r#type: "button",
                                disabled: !can_update,
                                onclick: {
                                    let form = form;
                                    move |_| {
                                        let document = match form.read().to_document() {
                                            Ok(document) => document,
                                            Err(()) => {
                                                notice.set(Some(ConfigNotice::InvalidForm));
                                                return;
                                            }
                                        };
                                        let compact = match serde_json::to_string(&document) {
                                            Ok(compact) => compact,
                                            Err(_) => {
                                                notice.set(Some(ConfigNotice::InvalidForm));
                                                return;
                                            }
                                        };
                                        let command_key =
                                            format!("{update_fingerprint}:{compact}");
                                        let current_command = match command() {
                                            Some(existing)
                                                if existing.signature == command_key => existing,
                                            _ => ConfigCommand {
                                                command_id: Uuid::new_v4(),
                                                fingerprint: update_fingerprint.clone(),
                                                document: document.clone(),
                                                signature: command_key,
                                            },
                                        };
                                        command.set(Some(current_command.clone()));
                                        let mut busy = busy;
                                        let mut notice = notice;
                                        let mut form = form;
                                        let mut checked_signature = checked_signature;
                                        let mut fingerprint = fingerprint;
                                        let mut restart_required = restart_required;
                                        let mut command = command;
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
                                                Ok(result) => match RuntimeConfigForm::from_document(
                                                    &result.document,
                                                ) {
                                                    Ok(next_form) => {
                                                        form.set(next_form);
                                                        checked_signature.set(String::new());
                                                        fingerprint
                                                            .set(Some(result.fingerprint));
                                                        restart_required
                                                            .set(result.restart_required);
                                                        command.set(None);
                                                        notice.set(Some(ConfigNotice::Saved));
                                                    }
                                                    Err(()) => notice
                                                        .set(Some(ConfigNotice::InvalidForm)),
                                                },
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
                                if busy() {
                                    {t!("admin-action-working")}
                                } else {
                                    {t!("admin-runtime-update")}
                                }
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
fn AdminRuntimeConfigFact(detail: String, state: Option<bool>, label: String) -> Element {
    rsx! {
        article {
            div { class: "admin-config-fact-heading",
                strong { "{label}" }
                if let Some(enabled) = state {
                    span {
                        class: if enabled {
                            "admin-config-fact-enabled"
                        } else {
                            "admin-config-fact-disabled"
                        },
                        if enabled {
                            {t!("admin-runtime-enabled")}
                        } else {
                            {t!("admin-runtime-disabled")}
                        }
                    }
                }
            }
            p { title: detail.clone(), "{detail}" }
        }
    }
}

fn notice_text(notice: &ConfigNotice) -> String {
    match notice {
        ConfigNotice::Validated => t!("admin-runtime-validated"),
        ConfigNotice::Saved => t!("admin-runtime-action-success"),
        ConfigNotice::InvalidForm => t!("admin-runtime-invalid-form"),
        ConfigNotice::ActionError(code) => admin_api_error_message(code),
        ConfigNotice::Conflict => t!("admin-runtime-conflict"),
    }
}
