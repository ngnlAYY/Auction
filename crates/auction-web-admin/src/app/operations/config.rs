//! Admin runtime 配置页面。
//! 页面用类型化表单编辑服务端下发的完整非敏感投影；候选必须先经过服务端规范化校验，
//! 更新再以 fingerprint 和可重试的 command ID 作为版本与幂等边界。成功更新会由服务端
//! 原子写入 runtime TOML，角色启动配置、公共配置、密钥和命令账本永远不进入浏览器。

#[path = "config/draft.rs"]
mod draft;
#[path = "config/early_bird.rs"]
mod early_bird;
#[path = "config/form.rs"]
mod form;
mod hot_fields {
    include!("config/hot_fields.rs");
}
#[path = "config/model.rs"]
mod model;
#[path = "config/operational_fields.rs"]
mod operational_fields;
#[path = "config/review.rs"]
mod review;
mod runtime_switches;
#[path = "config/validation.rs"]
mod validation;
#[path = "config/worlds.rs"]
mod worlds;

use auction_web_shared::{AsyncState, AsyncStateKind, Dialog, legacy_t as t};
use dioxus::prelude::*;
use serde_json::Value;

use crate::api::{
    fetch_admin_runtime_config, update_admin_runtime_config, validate_admin_runtime_config,
};

use self::{form::RuntimeConfigFields, model::RuntimeConfigForm, worlds::AdminBusinessWorlds};
use super::admin_api_error_message;
use super::pending::{PendingCommandPanel, set_if_alive};

/// 在认证外壳内安装配置草稿与整页离开提示。
pub(super) fn use_config_draft() {
    draft::use_config_draft();
}

#[derive(Debug, Clone)]
struct ConfigCommand {
    fingerprint: String,
    document: Value,
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
    let draft::ConfigDraft {
        mut form,
        mut baseline,
        mut fingerprint,
        mut checked_signature,
        mut command,
        mut notice,
        mut restart_required,
        dirty,
        pending,
        mut refresh,
        validating,
    } = use_context::<draft::ConfigDraft>();
    let mut form_ready = use_signal(|| false);
    let mut issues = use_signal(Vec::<validation::FieldIssue>::new);
    use_context_provider(|| validation::FormIssues(issues));
    let busy = move || validating() || pending.is_busy("runtime-config");
    let frozen = move || validating() || pending.contains("runtime-config");
    // 写入是危险操作：点击「写入配置」只准备命令并打开确认对话框。
    let mut update_confirm_open = use_signal(|| false);
    let mut reload_confirm_open = use_signal(|| false);
    let config = use_resource(move || {
        let generation = refresh();
        async move { (generation, fetch_admin_runtime_config().await) }
    });

    // 重读期间 Resource 保留旧值；只有本轮完成快照能装配草稿，不能恢复已丢弃的旧基线。
    use_effect(move || {
        let generation = refresh();
        if config.state().cloned() != UseResourceState::Ready {
            return;
        }
        let snapshot = config.read_unchecked();
        let Some((loaded_generation, result)) = snapshot.as_ref() else { return };
        if *loaded_generation != generation {
            return;
        }
        if let Ok(value) = result {
            // 未决命令保留提交时的配置与基线，读取不能改变重试或确认的负载。
            if command.peek().is_some() && baseline.peek().is_some() {
                form_ready.set(true);
                return;
            }
            let Some(taxonomy) = value.equipment_taxonomy.as_ref() else {
                form_ready.set(false);
                notice.set(Some(ConfigNotice::InvalidForm));
                return;
            };
            // 返回本页时保留当前会话草稿；外部版本变化也不能静默覆盖尚未保存的输入。
            if baseline.peek().is_some() {
                // 词典可独立于 document fingerprint 更新，不覆盖草稿或制造脏状态。
                form.write().refresh_equipment_taxonomy(taxonomy);
                if let Some(initial) = baseline.write().as_mut() {
                    initial.refresh_equipment_taxonomy(taxonomy);
                }
                if fingerprint.peek().as_deref() == Some(value.fingerprint.as_str()) {
                    form_ready.set(true);
                    return;
                }
                if *dirty.peek() {
                    checked_signature.set(String::new());
                    notice.set(Some(ConfigNotice::Conflict));
                    form_ready.set(true);
                    return;
                }
            }
            match RuntimeConfigForm::from_response(value) {
                Ok(next_form) => {
                    baseline.set(Some(next_form.clone()));
                    form.set(next_form);
                    form_ready.set(true);
                    fingerprint.set(Some(value.fingerprint.clone()));
                    restart_required.set(value.restart_required);
                    checked_signature.set(String::new());
                    command.set(None);
                    notice.set(None);
                    issues.set(Vec::new());
                }
                Err(()) => {
                    form_ready.set(false);
                    notice.set(Some(ConfigNotice::InvalidForm));
                }
            }
        }
    });

    let mut discard_and_reload = move || {
        if frozen() {
            return;
        }
        baseline.set(None);
        form_ready.set(false);
        fingerprint.set(None);
        checked_signature.set(String::new());
        command.set(None);
        notice.set(None);
        issues.set(Vec::new());
        reload_confirm_open.set(false);
        refresh.set(refresh().saturating_add(1));
    };

    let snapshot = config.read_unchecked();
    let current = if config.state().cloned() == UseResourceState::Ready {
        snapshot
            .as_ref()
            .filter(|(generation, _)| *generation == refresh())
            .map(|(_, result)| result)
    } else {
        None
    };
    let content = match current {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("admin-runtime-loading"),
                detail: "",
            }
        },
        Some(Err(error)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: admin_api_error_message(&error.code),
                detail: "",
                retry_label: t!("admin-retry"),
                on_retry: move |_| refresh.set(refresh().saturating_add(1)),
            }
        },
        Some(Ok(_)) if !form_ready() => {
            if matches!(notice(), Some(ConfigNotice::InvalidForm)) {
                rsx! {
                    AsyncState {
                        kind: AsyncStateKind::Error,
                        title: t!("admin-runtime-invalid-form"),
                        detail: "",
                        retry_label: t!("admin-retry"),
                        on_retry: move |_| refresh.set(refresh().saturating_add(1)),
                    }
                }
            } else {
                rsx! {
                    AsyncState {
                        kind: AsyncStateKind::Loading,
                        title: t!("admin-runtime-loading"),
                        detail: "",
                    }
                }
            }
        }
        Some(Ok(snapshot)) => {
            let current_fingerprint = fingerprint().unwrap_or_else(|| snapshot.fingerprint.clone());
            // 编辑已清除校验签名时，不能保存，也无需为每次按键重建并序列化整个候选文档。
            let can_update = !frozen()
                && dirty()
                && !checked_signature.read().is_empty()
                && form
                    .read()
                    .signature()
                    .is_ok_and(|signature| signature == *checked_signature.read());
            let enabled_worlds = ["main", "isekai"]
                .into_iter()
                .filter(|world| form.read().boolean(&format!("worlds.{world}.enabled")))
                .count();
            let equipment_filter_enabled = form.read().boolean("equipment_filter.enabled");
            let update_fingerprint = current_fingerprint.clone();
            let config_changes = if update_confirm_open() {
                command
                    .read()
                    .as_ref()
                    .zip(baseline.read().as_ref())
                    .map(|(pending, original)| {
                        review::changes(original.source_document(), &pending.document)
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };

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
                        novalidate: true,
                        onsubmit: {
                            let form = form;
                            move |event| {
                                event.prevent_default();
                                if frozen() { return; }
                                notice.set(None);
                                let field_issues = validation::collect_issues(&form.read());
                                if !field_issues.is_empty() {
                                    issues.set(field_issues);
                                    validation::focus_summary();
                                    return;
                                }
                                issues.set(Vec::new());
                                let document = match form.read().to_document() {
                                    Ok(document) => document,
                                    Err(()) => {
                                        notice.set(Some(ConfigNotice::InvalidForm));
                                        return;
                                    }
                                };
                                let mut busy = validating;
                                let mut form = form;
                                let mut checked_signature = checked_signature;
                                let mut command = command;
                                let mut notice = notice;
                                busy.set(true);
                                dioxus::core::Runtime::current().in_scope(form.origin_scope(), || {
                                    spawn(async move {
                                    match validate_admin_runtime_config(document).await {
                                        Ok(result) => match RuntimeConfigForm::from_response(&result) {
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
                                });
                            }
                        },
                        if dirty() {
                            p { class: "admin-config-draft", role: "status", {t!("admin-runtime-unsaved")} }
                        }
                        if !issues.read().is_empty() {
                            validation::ErrorSummary { issues: issues() }
                        }
                        RuntimeConfigFields {
                            form,
                            disabled: frozen(),
                            on_dirty: move |_| {
                                if frozen() { return; }
                                checked_signature.set(String::new());
                                command.set(None);
                                notice.set(None);
                                if !issues.peek().is_empty() {
                                    issues.set(validation::collect_issues(&form.read()));
                                }
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
                                disabled: frozen(),
                                onclick: move |_| {
                                    if frozen() { return; }
                                    if dirty() { reload_confirm_open.set(true); }
                                    else { discard_and_reload(); }
                                },
                                {t!("admin-runtime-reload")}
                            }
                            button {
                                class: "admin-btn-secondary",
                                r#type: "submit",
                                disabled: frozen(),
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
                                        if frozen() { return; }
                                        let document = match form.read().to_document() {
                                            Ok(document) => document,
                                            Err(()) => {
                                                notice.set(Some(ConfigNotice::InvalidForm));
                                                return;
                                            }
                                        };
                                        command.set(Some(ConfigCommand {
                                            fingerprint: update_fingerprint.clone(),
                                            document,
                                        }));
                                        // 危险写入不直接提交：先打开确认对话框展示变更摘要。
                                        notice.set(None);
                                        update_confirm_open.set(true);
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
                    // 写入确认对话框放在表单外，避免 dialog 语义嵌进 <form>。
                    if update_confirm_open() {
                            if let Some(pending_command) = command() {
                                Dialog {
                                    title: t!("admin-runtime-update-confirm-title"),
                                    close_label: t!("admin-users-action-cancel"),
                                    busy: busy(),
                                    on_close: move |_| update_confirm_open.set(false),
                                    p { class: "admin-panel__note",
                                        {t!(
                                            "admin-runtime-update-confirm-summary",
                                            fingerprint: pending_command.fingerprint.clone(),
                                            fields: config_changes.len() as i64
                                        )}
                                    }
                                    review::ConfigChangeList { changes: config_changes.clone() }
                                    div { class: "admin-shell__dialog-actions",
                                        button {
                                            class: "admin-btn-danger",
                                            r#type: "button",
                                            disabled: frozen(),
                                            onclick: move |_| {
                                                if frozen() {
                                                    return;
                                                }
                                                let Some(current_command) = command() else {
                                                    update_confirm_open.set(false);
                                                    return;
                                                };
                                                update_confirm_open.set(false);
                                                notice.set(None);
                                                pending.submit(
                                                    "runtime-config".to_owned(),
                                                    t!("admin-runtime-update"),
                                                    move |command_id| {
                                                        let current_command = current_command.clone();
                                                        async move {
                                                            let result = update_admin_runtime_config(
                                                                command_id,
                                                                current_command.fingerprint,
                                                                current_command.document,
                                                            )
                                                            .await
                                                            .map_err(|error| error.code)?;
                                                            let next_form = RuntimeConfigForm::from_response(&result)
                                                                .map_err(|()| "invalid_response".to_owned())?;
                                                            Ok((result, next_form))
                                                        }
                                                    },
                                                    move |(result, next_form)| {
                                                        set_if_alive(baseline, Some(next_form.clone()));
                                                        set_if_alive(form, next_form);
                                                        set_if_alive(checked_signature, String::new());
                                                        set_if_alive(fingerprint, Some(result.fingerprint));
                                                        set_if_alive(restart_required, result.restart_required);
                                                        set_if_alive(command, None);
                                                        set_if_alive(notice, Some(ConfigNotice::Saved));
                                                    },
                                                    move |code| {
                                                        set_if_alive(notice, Some(ConfigNotice::ActionError(code)));
                                                    },
                                                );
                                            },
                                            if busy() {
                                                {t!("admin-action-working")}
                                            } else {
                                                {t!("admin-users-action-confirm")}
                                            }
                                        }
                                        button {
                                            class: "admin-btn-secondary",
                                            r#type: "button",
                                            disabled: busy(),
                                            onclick: move |_| update_confirm_open.set(false),
                                            {t!("admin-users-action-cancel")}
                                        }
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
            AdminBusinessWorlds {}
            runtime_switches::RuntimeSwitchCard {}
            h2 { class: "admin-config-page__runtime-title", {t!("admin-runtime-document-title")} }
            PendingCommandPanel { owner: pending, session_scoped: true }
            {content}
            if reload_confirm_open() {
                Dialog {
                    title: t!("admin-runtime-discard-title"),
                    close_label: t!("admin-runtime-keep-editing"),
                    on_close: move |_| reload_confirm_open.set(false),
                    p { {t!("admin-runtime-discard-detail")} }
                    div { class: "admin-shell__dialog-actions",
                        button { class: "admin-btn-secondary", r#type: "button",
                            onclick: move |_| reload_confirm_open.set(false),
                            {t!("admin-runtime-keep-editing")}
                        }
                        button { class: "admin-btn-danger", r#type: "button",
                            disabled: frozen(),
                            onclick: move |_| discard_and_reload(),
                            {t!("admin-runtime-discard-reload")}
                        }
                    }
                }
            }
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
