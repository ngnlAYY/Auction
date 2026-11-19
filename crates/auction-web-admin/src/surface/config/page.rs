//! 运行配置页面：草稿、校验、保存与事实展示。

use auction_contracts::AdminWorkerRestartStateDto;
use auction_web_shared::{AsyncState, AsyncStateKind, Dialog, t};
use dioxus::prelude::*;
use serde_json::Value;

use super::form::RuntimeConfigFields;
use super::model::RuntimeConfigForm;
use super::reading::{ConfigReadStatus, use_config_read};
use super::worlds::AdminBusinessWorlds;
use super::{draft, mail_templates, review, runtime_switches, validation};
use crate::api::update_admin_runtime_config;
use crate::surface::admin_api_error_message;
use crate::surface::pending::{
    CommandDelivery, CommandReceipt, PendingCommandPanel, set_if_alive, use_pending_commands,
};

/// 在认证外壳内安装配置草稿与整页离开提示。
pub(crate) fn use_config_draft() {
    draft::use_config_draft();
}

#[derive(Debug, Clone)]
pub(super) struct ConfigCommand {
    pub(super) fingerprint: String,
    pub(super) document: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ConfigNotice {
    Validated,
    Saved,
    InvalidForm,
    ActionError(String),
    Conflict,
}

/// 加载并展示 Admin runtime 非敏感配置。
pub(crate) fn page() -> Element {
    rsx! {
        section {
            class: "admin-config-page",
            aria_labelledby: "config-title",
            h1 { id: "config-title", {t!("page-config-title")} }
            p { class: "admin-config-intro", {t!("admin-runtime-note")} }
            AdminBusinessWorlds {}
            runtime_switches::RuntimeSwitchCard {}
            mail_templates::MailTemplateCard {}
            RuntimeConfigDocument {}
        }
    }
}

#[component]
fn RuntimeConfigDocument() -> Element {
    let config_draft = use_context::<draft::ConfigDraft>();
    let draft::ConfigDraft {
        form,
        mut baseline,
        mut fingerprint,
        mut checked_signature,
        mut command,
        mut notice,
        restart_required,
        dirty,
        mut refresh,
        validating,
    } = config_draft;
    let pending = use_pending_commands(refresh, |entity| entity == "runtime-config");
    let mut issues = use_signal(Vec::<validation::FieldIssue>::new);
    use_context_provider(|| validation::FormIssues(issues));
    let busy = move || validating() || pending.is_busy("runtime-config");
    let frozen = move || validating() || pending.contains("runtime-config");
    // 写入是危险操作：点击「写入配置」只准备命令并打开展示变更摘要的确认对话框。
    let mut update_confirm_open = use_signal(|| false);
    let mut reload_confirm_open = use_signal(|| false);
    let (editor_mounted, completed_read) = use_config_read(config_draft, pending, issues);

    let mut discard_and_reload = move || {
        if frozen() {
            return;
        }
        baseline.set(None);
        fingerprint.set(None);
        checked_signature.set(String::new());
        command.set(None);
        notice.set(None);
        issues.set(Vec::new());
        reload_confirm_open.set(false);
        refresh.set(refresh().saturating_add(1));
    };

    let read_generation = refresh();
    let completed = completed_read.read();
    let current = completed.as_ref().filter(|read| read.generation == read_generation);
    let read_status = current.map(|read| &read.status);
    let read_requires_restart = current.is_some_and(|read| read.restart_required);
    let worker_restart_state = completed.as_ref().and_then(|read| read.worker_restart_state);
    let read_feedback = match read_status {
        Some(ConfigReadStatus::Ready(_)) => None,
        Some(ConfigReadStatus::Conflict) => {
            Some((AsyncStateKind::Error, notice_text(&ConfigNotice::Conflict)))
        }
        Some(ConfigReadStatus::Error(code)) => {
            Some((AsyncStateKind::Error, admin_api_error_message(code)))
        }
        Some(ConfigReadStatus::InvalidForm) => {
            Some((AsyncStateKind::Error, t!("admin-runtime-invalid-form")))
        }
        None => Some((AsyncStateKind::Loading, t!("admin-runtime-loading"))),
    };
    let read_panel = match &read_feedback {
        Some((_, title)) if matches!(read_status, Some(ConfigReadStatus::Conflict)) => rsx! {
            p { class: "admin-panel__note action-feedback action-feedback--error",
                role: "alert",
                "{title}"
            }
        },
        Some((kind, title)) => rsx! {
            AsyncState {
                kind: *kind,
                title: title.clone(),
                detail: "",
                retry_label: (*kind == AsyncStateKind::Error).then(|| t!("admin-retry")),
                on_retry: move |_| refresh.set(refresh().saturating_add(1)),
            }
        },
        None => VNode::empty(),
    };
    let content = match fingerprint().filter(|_| editor_mounted()) {
        Some(current_fingerprint) => {
            let editor_key = current_fingerprint.clone();
            let matches_baseline = current
                .is_some_and(|read| read.matches_baseline(read_generation, &current_fingerprint));
            // 点击入口只允许操作渲染时的读取世代和基线。
            let has_current_baseline = {
                let expected = current_fingerprint.clone();
                move || {
                    matches_baseline
                        && refresh.try_peek().is_ok_and(|generation| *generation == read_generation)
                        && fingerprint
                            .try_peek()
                            .is_ok_and(|current| current.as_ref() == Some(&expected))
                        && completed_read.try_peek().is_ok_and(|current| {
                            current.as_ref().is_some_and(|read| {
                                read.matches_baseline(read_generation, &expected)
                            })
                        })
                }
            };
            // 编辑已清除校验签名时，不能保存，也无需为每次按键重建并序列化整个候选文档。
            let can_update = matches_baseline
                && !frozen()
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
            let validation_fingerprint = current_fingerprint.clone();
            let can_confirm = matches_baseline
                && update_confirm_open()
                && !checked_signature.read().is_empty()
                && command
                    .read()
                    .as_ref()
                    .is_some_and(|draft| draft.fingerprint == current_fingerprint);
            let config_changes = if update_confirm_open() {
                command.read().as_ref().zip(baseline.read().as_ref()).map_or_default(
                    |(pending, original)| {
                        review::changes(original.source_document(), &pending.document)
                    },
                )
            } else {
                Vec::new()
            };
            let requires_restart = config_changes.iter().any(review::ConfigChange::needs_restart);
            let validate_label =
                if busy() { t!("admin-action-working") } else { t!("admin-runtime-validate") };
            let update_label =
                if busy() { t!("admin-action-working") } else { t!("admin-runtime-update") };
            let confirm_label =
                if busy() { t!("admin-action-working") } else { t!("admin-users-action-confirm") };
            let confirmation_read_feedback = if let Some((_, message)) = read_feedback.as_ref() {
                message.clone()
            } else if can_confirm {
                String::new()
            } else {
                notice_text(&ConfigNotice::Conflict)
            };
            let validation_is_current = has_current_baseline.clone();
            let confirmation_is_current = has_current_baseline.clone();

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
                            detail: current_fingerprint,
                            state: None,
                            label: t!("admin-runtime-fingerprint-label"),
                        }
                    }
                    form {
                        class: "admin-runtime-config__form admin-config-form",
                        novalidate: true,
                        onsubmit: {
                            move |event| {
                                event.prevent_default();
                                if frozen() || !validation_is_current() { return; }
                                checked_signature.set(String::new());
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
                                let Ok(signature) = serde_json::to_string(&document) else {
                                    notice.set(Some(ConfigNotice::InvalidForm));
                                    return;
                                };
                                config_draft.validate(validation_fingerprint.clone(), document, signature);
                            }
                        },
                        if dirty() {
                            p { class: "admin-config-draft", role: "status", {t!("admin-runtime-unsaved")} }
                        }
                        if !issues.read().is_empty() {
                            validation::ErrorSummary { issues: issues() }
                        }
                        // 以已接受的快照指纹重建字段控件，避免继承上一份配置的本地编辑与播报状态。
                        for editor_key in [editor_key] {
                            RuntimeConfigFields {
                                key: "{editor_key}",
                                form,
                                disabled: frozen(),
                                on_dirty: move |_| {
                                    if frozen() { return; }
                                    checked_signature.set(String::new());
                                    command.set(None);
                                    notice.set(None);
                                    if !issues.peek().is_empty() {
                                        // 输入可以纠正已报告的错误，新错误等离焦或再次提交。
                                        let remaining = {
                                            let previous = issues.peek();
                                            validation::collect_issues(&form.read()).into_iter()
                                                .filter(|issue| previous.iter().any(|old| old.path == issue.path))
                                                .collect()
                                        };
                                        issues.set(remaining);
                                    }
                                },
                            }
                        }
                        if let Some(current_notice) = notice().filter(|notice| match notice {
                            ConfigNotice::Saved => false,
                            ConfigNotice::Validated => matches_baseline && !checked_signature.read().is_empty(),
                            ConfigNotice::Conflict => !matches!(read_status, Some(ConfigReadStatus::Conflict)),
                            _ => true,
                        }) {
                            p {
                                class: if current_notice == ConfigNotice::Validated {
                                    "admin-panel__note action-feedback action-feedback--success"
                                } else {
                                    "admin-panel__note action-feedback action-feedback--error"
                                },
                                role: if current_notice == ConfigNotice::Validated { "status" } else { "alert" },
                                {notice_text(&current_notice)}
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
                                    else { refresh.set(refresh().saturating_add(1)); }
                                },
                                {t!("admin-runtime-reload")}
                            }
                            button {
                                class: "admin-btn-secondary",
                                r#type: "submit",
                                disabled: frozen() || !matches_baseline,
                                "{validate_label}"
                            }
                            button {
                                class: "admin-btn-primary",
                                r#type: "button",
                                disabled: !can_update,
                                onclick: {
                                    move |_| {
                                        if frozen() || !has_current_baseline() { return; }
                                        let document = match form.read().to_document() {
                                            Ok(document) => document,
                                            Err(()) => {
                                                notice.set(Some(ConfigNotice::InvalidForm));
                                                return;
                                            }
                                        };
                                        if !serde_json::to_string(&document)
                                            .is_ok_and(|signature| signature == *checked_signature.peek())
                                        {
                                            return;
                                        }
                                        command.set(Some(ConfigCommand {
                                            fingerprint: update_fingerprint.clone(),
                                            document,
                                        }));
                                        notice.set(None);
                                        update_confirm_open.set(true);
                                    }
                                },
                                "{update_label}"
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
                                            fingerprint: pending_command.fingerprint,
                                            fields: config_changes.len() as i64
                                        )}
                                    }
                                    review::ConfigChangeList { changes: config_changes, pending_restart: restart_required() }
                                    div { class: "admin-shell__dialog-actions",
                                        button {
                                            class: "admin-btn-danger",
                                            r#type: "button",
                                            disabled: frozen() || !can_confirm,
                                            onclick: move |_| {
                                                if frozen() || !update_confirm_open()
                                                    || !confirmation_is_current() || checked_signature.peek().is_empty()
                                                {
                                                    return;
                                                }
                                                let Some(current_command) = command() else {
                                                    update_confirm_open.set(false);
                                                    return;
                                                };
                                                if fingerprint.peek().as_deref() != Some(current_command.fingerprint.as_str()) {
                                                    return;
                                                }
                                                update_confirm_open.set(false);
                                                notice.set(None);
                                                pending.submit_with_receipt(
                                                    "runtime-config".to_owned(),
                                                    t!("admin-runtime-update"),
                                                    move |command_id| {
                                                        let current_command = current_command.clone();
                                                        async move {
                                                            let result = update_admin_runtime_config(
                                                                command_id,
                                                                current_command.fingerprint,
                                                                current_command.document,
                                                            ).await.map_err(|error| error.code)?;
                                                            let next_form = RuntimeConfigForm::from_response(&result)
                                                                .map_err(|()| "invalid_response".to_owned())?;
                                                            Ok((result, next_form))
                                                        }
                                                    },
                                                    move |(result, next_form), source_alive| {
                                                        let restart = result.restart_required && requires_restart;
                                                        // 写入回执同时推进会话草稿的版本与读取世代。
                                                        config_draft.apply_saved_config(result, next_form, requires_restart);
                                                        if source_alive {
                                                            set_if_alive(notice, Some(ConfigNotice::Saved));
                                                        }
                                                        CommandDelivery::reported(source_alive, || restart.then(|| CommandReceipt::Text(t!("admin-runtime-restart-required"))))
                                                    },
                                                    move |code| {
                                                        set_if_alive(notice, Some(ConfigNotice::ActionError(code)));
                                                    },
                                                );
                                            },
                                            "{confirm_label}"
                                        }
                                        button {
                                            class: "admin-btn-secondary",
                                            r#type: "button",
                                            disabled: busy(),
                                            onclick: move |_| update_confirm_open.set(false),
                                            {t!("admin-users-action-cancel")}
                                        }
                                    }
                                    // 常驻播报区域随本轮读取更新，取消后可在页面上重试。
                                    div { role: "status", aria_atomic: "true", "{confirmation_read_feedback}" }
                                }
                            }
                        }
                }
            }
        }
        None => VNode::empty(),
    };

    rsx! {
            h2 { class: "admin-config-page__runtime-title", {t!("admin-runtime-document-title")} }
            PendingCommandPanel { owner: pending }
            div { role: "status",
                if matches!(notice(), Some(ConfigNotice::Saved)) {
                    p { class: "admin-panel__note action-feedback action-feedback--success",
                        {notice_text(&ConfigNotice::Saved)}
                    }
                }
                if restart_required() || read_requires_restart {
                    p { class: "admin-panel__note admin-runtime-config__restart",
                        {t!("admin-runtime-restart-required")}
                    }
                }
            }
            div { role: "status",
                if let Some(state) = worker_restart_state {
                    p {
                        class: if state == AdminWorkerRestartStateDto::RestartRequired {
                            "admin-panel__note admin-runtime-config__restart"
                        } else {
                            "admin-panel__note"
                        },
                        {match state {
                            AdminWorkerRestartStateDto::RestartRequired => t!("admin-runtime-worker-restart-required"),
                            AdminWorkerRestartStateDto::Consistent => t!("admin-runtime-worker-restart-consistent"),
                            AdminWorkerRestartStateDto::Unknown => t!("admin-runtime-worker-restart-unknown"),
                        }}
                    }
                }
            }
            {content}
            {read_panel}
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

#[component]
pub(super) fn AdminRuntimeConfigFact(
    detail: String,
    state: Option<bool>,
    label: String,
) -> Element {
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
            p { title: detail, "{detail}" }
        }
    }
}

pub(super) fn notice_text(notice: &ConfigNotice) -> String {
    match notice {
        ConfigNotice::Validated => t!("admin-runtime-validated"),
        ConfigNotice::Saved => t!("admin-runtime-action-success"),
        ConfigNotice::InvalidForm => t!("admin-runtime-invalid-form"),
        ConfigNotice::ActionError(code) => admin_api_error_message(code),
        ConfigNotice::Conflict => t!("admin-runtime-conflict"),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use auction_contracts::{
        AdminRuntimeConfigResponse, AdminRuntimeConfigUpdateRequest, AdminWorkerRestartStateDto,
        ApiResponse,
    };
    use dioxus::core::{
        AttributeValue, DynamicNode, ElementId, Mutation, ScopeId, TemplateAttribute, TemplateNode,
        VirtualDom,
    };
    use dioxus::prelude::*;
    use serde_json::json;

    use super::draft::{ConfigDraft, ConfigValidationStub, use_config_draft};
    use super::{ConfigCommand, ConfigNotice, RuntimeConfigDocument};
    use crate::api::{AdminApiFailure, stub_runtime_config};
    use crate::surface::auctions::draft_page_tests::{
        RecordRead, assert_field, button, click_button, has_live_text, mount_writer, settle_writer,
        submit_button_disabled, writer_form,
    };
    use crate::surface::config::model::RuntimeConfigForm;
    use crate::surface::config::reading::ConfigReadGate;
    use crate::surface::config::review::{ConfigChange, changes};
    use crate::surface::realtime::{TestPage, text_attributes};
    use crate::surface::test_events::{dispatch_click_event, dispatch_form_event};

    fn draft_harness() -> Element {
        use_config_draft();
        VNode::empty()
    }

    fn find_in_tree<T>(
        dom: &VirtualDom,
        node: &VNode,
        find: &impl Fn(&VNode) -> Option<T>,
    ) -> Option<T> {
        find(node).or_else(|| {
            node.dynamic_nodes.iter().enumerate().find_map(|(index, child)| match child {
                DynamicNode::Component(component) => find_in_tree(
                    dom,
                    component.mounted_scope(index, node, dom)?.try_root_node()?,
                    find,
                ),
                DynamicNode::Fragment(children) => {
                    children.iter().find_map(|child| find_in_tree(dom, child, find))
                }
                _ => None,
            })
        })
    }

    fn assert_invalid_field(page: &TestPage, input: ElementId, description: &str) {
        let dom = page.dom();
        for (name, expected) in [("aria-invalid", "true"), ("aria-describedby", description)] {
            let found = find_in_tree(dom, dom.base_scope().root_node(), &|node| {
                node.dynamic_attrs.iter().enumerate().any(|(index, attrs)| {
                    node.mounted_dynamic_attribute(index, dom) == Some(input)
                        && attrs.iter().any(|attr| attr.name == name
                            && matches!(&attr.value, AttributeValue::Text(text) if text == expected))
                }).then_some(())
            });
            assert!(found.is_some(), "原数值输入须保留 {name}={expected}");
        }
    }

    fn has_dialog_status(page: &TestPage, expected: &str) -> bool {
        let dom = page.dom();
        find_in_tree(dom, dom.base_scope().root_node(), &|node| {
            node.dynamic_nodes.iter().enumerate().find_map(|(index, child)| {
                let DynamicNode::Component(component) = child else { return None };
                if !component.name.ends_with("::Dialog") {
                    return None;
                }
                let dialog = component.mounted_scope(index, node, dom)?.try_root_node()?;
                find_in_tree(dom, dialog, &|node| {
                    node.dynamic_nodes.iter().zip(node.template.node_paths).find_map(
                        |(child, path)| {
                            if !matches!(child, DynamicNode::Text(text) if text.value == expected) {
                                return None;
                            }
                            let (_, parent) = path.split_last()?;
                            let (&root, rest) = parent.split_first()?;
                            let template = rest.iter().try_fold(
                                &node.template.roots[usize::from(root)],
                                |template, &child| match template {
                                    TemplateNode::Element { children, .. } => {
                                        children.get(usize::from(child))
                                    }
                                    _ => None,
                                },
                            )?;
                            let TemplateNode::Element { attrs, .. } = template else { return None };
                            attrs
                                .iter()
                                .any(|attr| {
                                    matches!(
                                        attr,
                                        TemplateAttribute::Static {
                                            name: "role",
                                            value: "status",
                                            ..
                                        }
                                    )
                                })
                                .then_some(())
                        },
                    )
                })
            })
        })
        .is_some()
    }

    fn template_text_attribute<'a>(
        node: &'a VNode,
        attrs: &[TemplateAttribute],
        name: &str,
    ) -> Option<&'a str> {
        attrs.iter().find_map(|attr| match attr {
            TemplateAttribute::Static { name: key, value, .. } if *key == name => Some(*value),
            TemplateAttribute::Dynamic { id } => {
                node.dynamic_attrs[*id].iter().find_map(|attr| match &attr.value {
                    AttributeValue::Text(value) if attr.name == name => Some(value.as_str()),
                    _ => None,
                })
            }
            _ => None,
        })
    }

    fn worker_status_region(page: &TestPage, expected: &str, warning: bool) -> ElementId {
        let dom = page.dom();
        find_in_tree(dom, dom.base_scope().root_node(), &|node| {
            node.template.roots.iter().enumerate().find_map(|(index, root)| {
                let TemplateNode::Element { attrs, children, .. } = root else { return None };
                if template_text_attribute(node, attrs, "role") != Some("status") {
                    return None;
                }
                let classes = children.iter().find_map(|child| {
                    let TemplateNode::Dynamic { id } = child else { return None };
                    let DynamicNode::Fragment(lines) = &node.dynamic_nodes[*id] else {
                        return None;
                    };
                    lines.iter().find_map(|line| {
                        let [TemplateNode::Element { attrs, children, .. }] = line.template.roots
                        else {
                            return None;
                        };
                        let contains_text = children.iter().any(|child| {
                            matches!(child, TemplateNode::Dynamic { id }
                                if matches!(&line.dynamic_nodes[*id],
                                    DynamicNode::Text(text) if text.value == expected))
                        });
                        contains_text.then(|| {
                            template_text_attribute(line, attrs, "class").unwrap_or_default()
                        })
                    })
                })?;
                assert!(classes.split_ascii_whitespace().any(|class| class == "admin-panel__note"));
                assert_eq!(
                    classes
                        .split_ascii_whitespace()
                        .any(|class| class == "admin-runtime-config__restart"),
                    warning,
                    "Worker 状态行的重启警示样式应与状态一致",
                );
                node.mounted_root(index, dom)
            })
        })
        .unwrap_or_else(|| panic!("状态播报容器缺少文本「{expected}」"))
    }

    fn assert_status_region_kept(region: ElementId, edits: &[Mutation]) {
        // 挂载 ID 可以复用，须同时确认渲染指令没有移除或重新挂载原容器。
        assert!(
            edits.iter().all(|edit| !matches!(edit,
                Mutation::Remove { id } | Mutation::ReplaceWith { id, .. }
                    | Mutation::LoadTemplate { id, .. } | Mutation::AssignId { id, .. }
                    if *id == region
            )),
            "后台读取须保留原状态播报容器"
        );
    }

    struct ConfigRequests {
        projection: RefCell<AdminRuntimeConfigResponse>,
        reads: Cell<usize>,
        writes: Cell<usize>,
    }

    impl ConfigRequests {
        fn read(&self) -> Result<AdminRuntimeConfigResponse, AdminApiFailure> {
            let previous = self.reads.replace(self.reads.get() + 1);
            if previous == 1 {
                return Err(AdminApiFailure::from_http_body(
                    503,
                    r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
                ));
            }
            Ok(self.projection.borrow().clone())
        }

        fn save(&self, command: AdminRuntimeConfigUpdateRequest) -> AdminRuntimeConfigResponse {
            self.writes.set(self.writes.get() + 1);
            let mut saved = self.projection.borrow_mut();
            assert!(!command.command_id.is_nil());
            assert_eq!(command.expected_fingerprint, saved.fingerprint);
            assert_eq!(command.document["operations"]["forum"]["lease_duration_seconds"], 600);
            saved.document = command.document;
            saved.fingerprint = format!("sha256:{}", "1".repeat(64));
            let mut response = saved.clone();
            response.restart_required = true;
            response.worker_restart_state = None;
            response
        }
    }

    #[test]
    fn keyed_config_editor_reports_a_new_snapshot_historical_number_immediately() {
        const PATH: &str = "operations.worker.poll_interval_seconds";
        let mut response: ApiResponse<AdminRuntimeConfigResponse> =
            serde_json::from_str(include_str!(
                "../../../../auction-contracts/tests/fixtures/admin/runtime-config-default.json"
            ))
            .unwrap();
        response.data.document["operations"]["worker"]["managed"] = json!(true);
        let snapshot = Rc::new(RefCell::new(response.data));
        let (mut page, initial) = mount_writer({
            let snapshot = snapshot.clone();
            move || {
                use_config_draft();
                use_hook({
                    let snapshot = snapshot.clone();
                    move || {
                        stub_runtime_config(
                            move || Ok(snapshot.borrow().clone()),
                            |_| panic!("只重读配置"),
                        );
                    }
                });
                let mut refresh = use_context::<ConfigDraft>().refresh;
                rsx! {
                    button { onclick: move |_| refresh.set(refresh() + 1),
                        {"重读测试快照".to_owned()}
                    }
                    RuntimeConfigDocument {}
                }
            }
        });
        let input = text_attributes(&initial, "data-runtime-number")
            .find_map(|(id, path)| (path == PATH).then_some(id))
            .unwrap();
        page.dispatch("input", input, "0");
        let blurred = page.dispatch("blur", input, "");
        assert!(text_attributes(&blurred, "aria-invalid").any(|entry| entry == (input, "true")));
        let corrected = page.dispatch("input", input, "60");
        assert!(text_attributes(&corrected, "aria-invalid").any(|entry| entry == (input, "false")));

        {
            let mut next = snapshot.borrow_mut();
            next.fingerprint = format!("sha256:{}", "2".repeat(64));
            next.document["operations"]["worker"]["poll_interval_seconds"] = json!(0);
        }
        // 同一编辑页后台重读，不走主动丢弃草稿后卸载编辑器的路径。
        let refreshed = click_button(&mut page, "重读测试快照");
        let rebuilt = text_attributes(&refreshed, "data-runtime-number")
            .find_map(|(id, path)| (path == PATH).then_some(id))
            .expect("新指纹须重建数字字段");
        assert!(text_attributes(&refreshed, "value").any(|entry| entry == (rebuilt, "0")));
        assert!(
            text_attributes(&refreshed, "aria-invalid").any(|entry| entry == (rebuilt, "true"))
        );
        let description = text_attributes(&refreshed, "aria-describedby")
            .find_map(|(id, value)| (id == rebuilt).then_some(value))
            .expect("历史越界值须立即关联字段旁错误");
        assert!(text_attributes(&refreshed, "id").any(|(_, id)| id == description));
    }

    #[test]
    fn background_reads_keep_the_edited_field_and_its_error() {
        const PATH: &str = "operations.worker.poll_interval_seconds";
        let mut response: ApiResponse<AdminRuntimeConfigResponse> =
            serde_json::from_str(include_str!(
                "../../../../auction-contracts/tests/fixtures/admin/runtime-config-default.json"
            ))
            .unwrap();
        response.data.document["operations"]["worker"]["managed"] = json!(true);
        let snapshot = Rc::new(RefCell::new(response.data));
        let gate = Rc::new(RecordRead::new());
        let (mut page, initial) = mount_writer({
            let snapshot = snapshot.clone();
            let gate = gate.clone();
            move || {
                use_config_draft();
                use_context_provider(|| ConfigReadGate(gate.clone()));
                use_hook({
                    let snapshot = snapshot.clone();
                    move || {
                        stub_runtime_config(
                            move || Ok(snapshot.borrow().clone()),
                            |_| panic!("只重读配置"),
                        );
                    }
                });
                let mut refresh = use_context::<ConfigDraft>().refresh;
                rsx! {
                    button { onclick: move |_| refresh.set(refresh() + 1),
                        {"后台重读".to_owned()}
                    }
                    RuntimeConfigDocument {}
                }
            }
        });
        let input = text_attributes(&initial, "data-runtime-number")
            .find_map(|(id, path)| (path == PATH).then_some(id))
            .unwrap();
        let unknown = "无法确认运行中的 Worker 是否已采用当前配置（没有可比较的在线 Worker，或其版本不提供这项信息）。";
        let consistent = "运行中的 Worker 已采用当前配置文件里需要重启才生效的设置。";
        let region = worker_status_region(&page, unknown, false);
        page.dispatch("input", input, "0");
        let blurred = page.dispatch("blur", input, "");
        assert!(text_attributes(&blurred, "aria-invalid").any(|entry| entry == (input, "true")));

        let description = text_attributes(&blurred, "aria-describedby")
            .find_map(|(id, value)| (id == input).then_some(value))
            .expect("离焦后数值错误须关联到输入控件");
        let pending = click_button(&mut page, "后台重读");
        assert_eq!(gate.started(), 1);
        assert!(has_live_text(&page, "正在加载运行时配置……"));
        assert_eq!(worker_status_region(&page, unknown, false), region);
        assert_status_region_kept(region, &pending);
        assert_field(&page, input, "0", false);
        assert_invalid_field(&page, input, description);
        assert_eq!(submit_button_disabled(&page, "校验候选配置"), Some(true));
        {
            let mut next = snapshot.borrow_mut();
            next.equipment_taxonomy.as_mut().unwrap().revision = "test-2".to_owned();
            next.worker_restart_state = Some(AdminWorkerRestartStateDto::Consistent);
        }
        gate.respond(Ok(()));
        let same = settle_writer(&mut page);
        assert!(!has_live_text(&page, unknown));
        assert_eq!(worker_status_region(&page, consistent, false), region);
        assert_status_region_kept(region, &same);
        assert!(!text_attributes(&same, "data-runtime-number").any(|(_, path)| path == PATH));
        assert_field(&page, input, "0", false);
        assert_invalid_field(&page, input, description);
        assert_eq!(submit_button_disabled(&page, "校验候选配置"), Some(false));
        page.dom().in_scope(ScopeId::APP, || {
            let draft = consume_context::<ConfigDraft>();
            assert_eq!(draft.form.peek().equipment_taxonomy().unwrap().revision, "test-2");
            assert!((draft.dirty)());
        });

        snapshot.borrow_mut().fingerprint = format!("sha256:{}", "2".repeat(64));
        let pending = click_button(&mut page, "后台重读");
        assert_eq!(worker_status_region(&page, consistent, false), region);
        assert_status_region_kept(region, &pending);
        gate.respond(Ok(()));
        let changed = settle_writer(&mut page);
        assert_eq!(worker_status_region(&page, consistent, false), region);
        assert_status_region_kept(region, &changed);
        assert!(!text_attributes(&changed, "data-runtime-number").any(|(_, path)| path == PATH));
        let conflict = "配置已经变化，请重新加载后再次校验。";
        assert!(has_live_text(&page, conflict));
        assert_invalid_field(&page, input, description);
        assert_eq!(submit_button_disabled(&page, "校验候选配置"), Some(true));
        assert!(button(&page, "写入配置").unwrap().1);
        let corrected = page.dispatch("input", input, "60");
        assert!(text_attributes(&corrected, "aria-invalid").any(|entry| entry == (input, "false")));
        assert!(has_live_text(&page, conflict));
    }

    #[test]
    fn stale_config_callbacks_cannot_validate_or_write_until_the_baseline_is_current() {
        let mut response: ApiResponse<AdminRuntimeConfigResponse> =
            serde_json::from_str(include_str!(
                "../../../../auction-contracts/tests/fixtures/admin/runtime-config-default.json"
            ))
            .unwrap();
        let forum = &mut response.data.document["operations"]["forum"];
        forum["managed"] = json!(true);
        forum["lease_duration_seconds"] = json!(300);
        forum["item_timeout_seconds"] = json!(240);
        let requests = Rc::new(ConfigRequests {
            projection: RefCell::new(response.data),
            reads: Cell::new(0),
            writes: Cell::new(0),
        });
        let gate = Rc::new(RecordRead::new());
        let (mut page, initial) = mount_writer({
            let requests = requests.clone();
            let gate = gate.clone();
            move || {
                use_config_draft();
                use_context_provider(|| ConfigReadGate(gate.clone()));
                use_hook({
                    let reads = requests.clone();
                    let writes = requests.clone();
                    move || stub_runtime_config(move || reads.read(), move |cmd| writes.save(cmd))
                });
                rsx! { RuntimeConfigDocument {} }
            }
        });
        let form = writer_form(&initial);
        let mut draft = page.dom().in_scope(ScopeId::APP, consume_context::<ConfigDraft>);
        draft.form.write().set_input("operations.forum.lease_duration_seconds", "600".to_owned());
        let signature = draft.form.peek().signature().unwrap();
        draft.checked_signature.set(signature.clone());
        settle_writer(&mut page);

        let (save, disabled) = button(&page, "写入配置").unwrap();
        assert!(!disabled);
        let fingerprint = (draft.fingerprint)();
        draft.fingerprint.set(Some(format!("sha256:{}", "2".repeat(64))));
        // 未重绘前事件仍调用上一轮闭包，不能依赖按钮的 disabled 属性。
        dispatch_form_event(page.dom(), form, "submit", "");
        dispatch_click_event(page.dom(), save);
        assert!(!(draft.validating)());
        assert!((draft.command)().is_none());
        assert_eq!((draft.checked_signature)(), signature);
        draft.fingerprint.set(fingerprint);
        settle_writer(&mut page);

        click_button(&mut page, "写入配置");
        let (confirm, disabled) = button(&page, "确认").unwrap();
        assert!(!disabled);
        let frozen = (draft.command)().unwrap().document;
        draft.refresh.set(1);
        dispatch_form_event(page.dom(), form, "submit", "");
        dispatch_click_event(page.dom(), confirm);
        assert!(!(draft.validating)());
        assert_eq!((draft.checked_signature)(), signature);
        settle_writer(&mut page);
        assert_eq!(gate.started(), 1);
        for pending in [true, false] {
            let reason = if pending {
                "正在加载运行时配置……"
            } else {
                "所需服务暂时不可用。请稍后再试。"
            };
            if !pending {
                gate.respond(Ok(()));
                settle_writer(&mut page);
            }
            assert!(has_dialog_status(&page, reason));
            assert_eq!(submit_button_disabled(&page, "校验候选配置"), Some(true));
            let (save, save_disabled) = button(&page, "写入配置").unwrap();
            let (confirm, confirm_disabled) = button(&page, "确认").unwrap();
            assert!(save_disabled && confirm_disabled);
            dispatch_form_event(page.dom(), form, "submit", "");
            dispatch_click_event(page.dom(), save);
            dispatch_click_event(page.dom(), confirm);
            assert!(!(draft.validating)());
            settle_writer(&mut page);
            assert_eq!(requests.writes.get(), 0);
            assert_eq!((draft.command)().unwrap().document, frozen);
        }
        click_button(&mut page, "取消");
        assert!(button(&page, "确认").is_none());
        click_button(&mut page, "重试");
        gate.respond(Ok(()));
        settle_writer(&mut page);
        assert_eq!(submit_button_disabled(&page, "校验候选配置"), Some(false));
        click_button(&mut page, "写入配置");
        click_button(&mut page, "确认");
        assert_eq!(requests.writes.get(), 1);

        gate.respond(Ok(()));
        settle_writer(&mut page);
        draft.form.write().set_input("operations.forum.lease_duration_seconds", "900".to_owned());
        draft.checked_signature.set(draft.form.peek().signature().unwrap());
        settle_writer(&mut page);
        click_button(&mut page, "写入配置");
        requests.projection.borrow_mut().fingerprint = format!("sha256:{}", "3".repeat(64));
        draft.refresh.set((draft.refresh)() + 1);
        settle_writer(&mut page);
        gate.respond(Ok(()));
        settle_writer(&mut page);
        assert!(has_dialog_status(&page, "配置已经变化，请重新加载后再次校验。"));
        assert!(button(&page, "确认").unwrap().1);
        click_button(&mut page, "取消");
        assert!(button(&page, "确认").is_none());
        click_button(&mut page, "重新加载当前配置");
        assert!(button(&page, "放弃修改并重新加载").is_some());
        assert_eq!(requests.writes.get(), 1);
    }

    #[test]
    fn config_validation_receipt_follows_the_session_draft_across_navigation() {
        const PATH: &str = "operations.forum.lease_duration_seconds";
        let mut response: ApiResponse<AdminRuntimeConfigResponse> =
            serde_json::from_str(include_str!(
                "../../../../auction-contracts/tests/fixtures/admin/runtime-config-default.json"
            ))
            .unwrap();
        let forum = &mut response.data.document["operations"]["forum"];
        forum["managed"] = json!(true);
        forum["lease_duration_seconds"] = json!(300);
        forum["item_timeout_seconds"] = json!(240);
        for (change_fingerprint, change_draft, reject, return_before_receipt) in [
            (false, false, false, false),
            (false, false, true, true),
            (true, false, false, true),
            (false, true, true, true),
        ] {
            let validation = Rc::new(RecordRead::new());
            let snapshot = Rc::new(RefCell::new(response.data.clone()));
            let (mut page, initial) = mount_writer({
                let validation = validation.clone();
                let snapshot = snapshot.clone();
                move || {
                    use_config_draft();
                    use_context_provider(|| ConfigValidationStub(validation.clone()));
                    use_hook({
                        let snapshot = snapshot.clone();
                        move || {
                            stub_runtime_config(
                                move || Ok(snapshot.borrow().clone()),
                                |_| panic!("只预校验配置"),
                            );
                        }
                    });
                    let mut visible = use_signal(|| true);
                    rsx! {
                        button { onclick: move |_| visible.toggle(), {"切换配置页".to_owned()} }
                        if visible() { RuntimeConfigDocument {} }
                    }
                }
            });
            let form = writer_form(&initial);
            let mut draft = page.dom().in_scope(ScopeId::APP, consume_context::<ConfigDraft>);
            draft.form.write().set_input(PATH, "600".to_owned());
            let signature = draft.form.peek().signature().unwrap();
            page.dispatch("submit", form, "");
            assert_eq!(validation.started(), 1);
            assert!((draft.validating)());
            click_button(&mut page, "切换配置页");
            assert!(!has_live_text(&page, "正在处理……"));
            draft.refresh.set(1);
            if change_fingerprint {
                // 还原干净草稿，使返回后的真实 GET 采用外部新版本。
                draft.form.set((draft.baseline)().unwrap());
                snapshot.borrow_mut().fingerprint = format!("sha256:{}", "2".repeat(64));
            }
            if change_draft {
                draft.form.write().set_input(PATH, "601".to_owned());
            }
            if return_before_receipt {
                click_button(&mut page, "切换配置页");
                assert_eq!(submit_button_disabled(&page, "正在处理……"), Some(true));
            }
            if change_fingerprint {
                assert_ne!(
                    (draft.fingerprint)().as_deref(),
                    Some(response.data.fingerprint.as_str())
                );
                draft.form.write().set_input(PATH, "600".to_owned());
                assert_eq!(draft.form.peek().signature().unwrap(), signature);
            }
            if reject {
                validation.respond_unavailable();
            } else {
                validation.respond(Ok(response.data.clone()));
            }
            settle_writer(&mut page);
            assert!(!(draft.validating)());
            if !return_before_receipt {
                click_button(&mut page, "切换配置页");
            }
            if change_fingerprint || change_draft {
                assert!((draft.notice)().is_none());
                assert!((draft.checked_signature)().is_empty());
            } else if reject {
                assert!(matches!((draft.notice)(), Some(ConfigNotice::ActionError(_))));
                assert!(has_live_text(&page, "所需服务暂时不可用。请稍后再试。"));
                assert!((draft.checked_signature)().is_empty());
            } else {
                assert_eq!((draft.notice)(), Some(ConfigNotice::Validated));
                assert_eq!((draft.checked_signature)(), signature);
            }
            assert_eq!(
                button(&page, "写入配置").unwrap().1,
                reject || change_fingerprint || change_draft
            );
        }
    }

    #[test]
    fn config_save_and_restart_receipt_remain_visible_when_the_read_after_save_fails() {
        let mut response: ApiResponse<AdminRuntimeConfigResponse> =
            serde_json::from_str(include_str!(
                "../../../../auction-contracts/tests/fixtures/admin/runtime-config-default.json"
            ))
            .unwrap();
        response.data.worker_restart_state = Some(AdminWorkerRestartStateDto::RestartRequired);
        let forum = &mut response.data.document["operations"]["forum"];
        forum["managed"] = json!(true);
        forum["lease_duration_seconds"] = json!(300);
        forum["item_timeout_seconds"] = json!(240);
        let requests = Rc::new(ConfigRequests {
            projection: RefCell::new(response.data),
            reads: Cell::new(0),
            writes: Cell::new(0),
        });
        let (mut page, _) = mount_writer({
            let requests = requests.clone();
            move || {
                use_config_draft();
                use_hook({
                    let reads = requests.clone();
                    let writes = requests.clone();
                    move || {
                        stub_runtime_config(
                            move || reads.read(),
                            move |command| writes.save(command),
                        );
                    }
                });
                let mut draft = use_context::<ConfigDraft>();
                rsx! {
                    button { onclick: move |_| {
                        draft.form.write().set_input("operations.forum.lease_duration_seconds", "600".to_owned());
                        draft.checked_signature.set(draft.form.peek().signature().unwrap());
                    }, {"准备已校验的候选".to_owned()} }
                    RuntimeConfigDocument {}
                }
            }
        });
        let saved = "运行时配置已写入 TOML 文件。";
        let restart = "请重启受影响的进程，以应用尚未生效的配置。";
        let worker_restart =
            "当前配置文件里有尚未被运行中的 Worker 采用的设置，重启 Worker 后生效。";
        let worker_consistent = "运行中的 Worker 已采用当前配置文件里需要重启才生效的设置。";
        let error = "所需服务暂时不可用。请稍后再试。";
        assert!(!has_live_text(&page, saved));
        assert!(!has_live_text(&page, restart));
        let region = worker_status_region(&page, worker_restart, true);
        click_button(&mut page, "准备已校验的候选");
        click_button(&mut page, "写入配置");
        let failed_read = click_button(&mut page, "确认");
        assert_eq!(requests.writes.get(), 1);
        assert_eq!(requests.reads.get(), 2);
        assert!(has_live_text(&page, error));
        assert!(button(&page, "写入配置").is_some_and(|(_, disabled)| disabled));
        assert!(has_live_text(&page, saved));
        assert!(has_live_text(&page, restart));
        assert_eq!(worker_status_region(&page, worker_restart, true), region);
        assert_status_region_kept(region, &failed_read);
        requests.projection.borrow_mut().worker_restart_state =
            Some(AdminWorkerRestartStateDto::Consistent);
        let retried = click_button(&mut page, "重试");
        assert_eq!(requests.reads.get(), 3);
        assert!(!has_live_text(&page, error));
        assert!(has_live_text(&page, saved));
        assert!(has_live_text(&page, restart));
        assert_eq!(worker_status_region(&page, worker_consistent, false), region);
        assert_status_region_kept(region, &retried);
        assert!(!has_live_text(&page, worker_restart));
    }

    #[test]
    fn hot_forum_timeout_save_preserves_the_previous_lease_restart_notice() {
        let mut response: ApiResponse<AdminRuntimeConfigResponse> =
            serde_json::from_str(include_str!(
                "../../../../auction-contracts/tests/fixtures/admin/runtime-config-default.json"
            ))
            .unwrap();
        let forum = &mut response.data.document["operations"]["forum"];
        forum["managed"] = json!(true);
        forum["lease_duration_seconds"] = json!(300);
        forum["item_timeout_seconds"] = json!(240);

        let mut dom = VirtualDom::new(draft_harness);
        dom.rebuild_to_vec();
        dom.in_scope(ScopeId::APP, || {
            let mut draft = consume_context::<ConfigDraft>();
            let startup = RuntimeConfigForm::from_response(&response.data).unwrap();
            draft.baseline.set(Some(startup.clone()));
            draft.form.set(startup);
            draft.fingerprint.set(Some(response.data.fingerprint.clone()));

            for (field, input, needs_restart, fingerprint_digit) in [
                ("lease_duration_seconds", 600, true, "1"),
                // 360 秒小于新租约，却与仍在运行的 300 秒启动租约不相容。
                ("item_timeout_seconds", 360, false, "2"),
            ] {
                assert_eq!((draft.restart_required)(), !needs_restart);
                let mut result = response.data.clone();
                let initial = (draft.baseline)().unwrap();
                result.document.clone_from(initial.source_document());
                result.document["operations"]["forum"][field] = json!(input);
                result.restart_required = true;
                result.fingerprint = format!("sha256:{}", fingerprint_digit.repeat(64));

                let changed = changes(initial.source_document(), &result.document);
                assert_eq!(changed.len(), 1);
                let requires_restart = changed.iter().any(ConfigChange::needs_restart);
                assert_eq!(requires_restart, needs_restart);
                let next_form = RuntimeConfigForm::from_response(&result).unwrap();
                draft.checked_signature.set("已校验的候选".to_owned());
                draft.command.set(Some(ConfigCommand {
                    fingerprint: (draft.fingerprint)().unwrap(),
                    document: result.document.clone(),
                }));

                draft.apply_saved_config(result.clone(), next_form, requires_restart);

                assert!((draft.restart_required)());
                assert_eq!((draft.baseline)().unwrap().source_document(), &result.document);
                assert_eq!(draft.form.read().source_document(), &result.document);
                assert_eq!((draft.fingerprint)(), Some(result.fingerprint));
                assert!((draft.checked_signature)().is_empty());
                assert!((draft.command)().is_none());
                assert!((draft.notice)().is_none());
            }
            draft.remember_restart_notice(false);
            assert!((draft.restart_required)(), "普通文件重读并非重启确认");
        });
    }
}
