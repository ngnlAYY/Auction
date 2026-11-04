//! 运行时配置的分组表单控件。

#[path = "form/equipment_colors.rs"]
mod equipment_colors;
#[path = "form/operations/mod.rs"]
mod operations;

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use super::{
    model::RuntimeConfigForm,
    validation::{self, FormIssues},
};

#[component]
pub(super) fn RuntimeConfigFields(
    form: Signal<RuntimeConfigForm>,
    disabled: bool,
    on_dirty: EventHandler<()>,
) -> Element {
    let listener_mode = form.read().input("listener.mode");
    let mailboxes = form.read().mailboxes();
    let can_add_mailbox = mailboxes.len() < 2;

    rsx! {
        div { class: "admin-runtime-fields",
            RuntimeSection { title: t!("admin-runtime-section-auction"), open: false,
                div { class: "admin-runtime-field-grid",
                    SwitchField { form, disabled, path: "allow_manual_early_open", label: t!("admin-runtime-manual-open"), on_dirty: on_dirty }
                    SwitchField { form, disabled, path: "allow_manual_early_settlement", label: t!("admin-runtime-manual-settlement"), on_dirty: on_dirty }
                    ListField { form, disabled, path: "bot_players", label: t!("admin-runtime-bot-players"), on_dirty: on_dirty }
                }
            }

            RuntimeSection { title: t!("admin-runtime-section-worlds"), open: false,
                div { class: "admin-runtime-world-grid",
                    RuntimeGroup { title: t!("admin-runtime-world-main"),
                        SwitchField { form, disabled, path: "worlds.main.enabled", label: t!("admin-runtime-world-enabled"), on_dirty: on_dirty }
                        SwitchField { form, disabled, path: "worlds.main.sender_allowlist_disabled", label: t!("admin-runtime-world-all-senders"), on_dirty: on_dirty }
                        ListField { form, disabled, path: "worlds.main.sender_allowlist", label: t!("admin-runtime-world-senders"), on_dirty: on_dirty }
                    }
                    RuntimeGroup { title: t!("admin-runtime-world-isekai"),
                        SwitchField { form, disabled, path: "worlds.isekai.enabled", label: t!("admin-runtime-world-enabled"), on_dirty: on_dirty }
                        SwitchField { form, disabled, path: "worlds.isekai.sender_allowlist_disabled", label: t!("admin-runtime-world-all-senders"), on_dirty: on_dirty }
                        ListField { form, disabled, path: "worlds.isekai.sender_allowlist", label: t!("admin-runtime-world-senders"), on_dirty: on_dirty }
                    }
                }
            }

            operations::OperationalConfigFields { form, disabled, on_dirty }

            RuntimeSection { title: t!("admin-runtime-section-equipment"), open: false,
                RuntimeGroup { title: t!("admin-runtime-equipment-filter-title"),
                    p { class: "admin-runtime-section-note", {t!("admin-runtime-quality-gate-note")} }
                    div { class: "admin-runtime-field-grid",
                        SwitchField { form, disabled, path: "equipment_filter.enabled", label: t!("admin-runtime-filter-enabled"), on_dirty: on_dirty }
                        NumberField { form, disabled, path: "equipment_filter.level.min", label: t!("admin-runtime-level-min"), minimum: "0", maximum: "1000", on_dirty: on_dirty }
                        NumberField { form, disabled, path: "equipment_filter.level.max", label: t!("admin-runtime-level-max"), minimum: "0", maximum: "1000", on_dirty: on_dirty }
                    }
                    div { class: "admin-runtime-field-grid admin-runtime-field-grid--lists",
                        ListField { form, disabled, path: "equipment_filter.qualities", label: t!("admin-runtime-filter-qualities"), on_dirty: on_dirty }
                        ListField { form, disabled, path: "equipment_filter.prefixes", label: t!("admin-runtime-filter-prefixes"), on_dirty: on_dirty }
                        ListField { form, disabled, path: "equipment_filter.suffixes", label: t!("admin-runtime-filter-suffixes"), on_dirty: on_dirty }
                        ListField { form, disabled, path: "equipment_filter.categories", label: t!("admin-runtime-filter-categories"), on_dirty: on_dirty }
                        ListField { form, disabled, path: "equipment_filter.equipment_types", label: t!("admin-runtime-filter-types"), on_dirty: on_dirty }
                    }
                }
                RuntimeGroup { title: t!("admin-runtime-equipment-colors-title"),
                    equipment_colors::EquipmentColorRulesField { form, disabled, on_dirty }
                }
                RuntimeGroup { title: t!("admin-runtime-equipment-source-title"),
                    div { class: "admin-runtime-field-grid",
                        SwitchField { form, disabled, path: "equipment_source.allowlist_enabled", label: t!("admin-runtime-source-allowlist"), on_dirty: on_dirty }
                        SwitchField { form, disabled, path: "equipment_source.require_tradeable", label: t!("admin-runtime-source-tradeable"), on_dirty: on_dirty }
                        NumberField { form, disabled, path: "equipment_source.maximum_response_bytes", label: t!("admin-runtime-source-max-bytes"), minimum: "65536", maximum: "8388608", on_dirty: on_dirty }
                        NumberField { form, disabled, path: "equipment_source.minimum_request_interval_millis", label: t!("admin-runtime-source-min-interval"), minimum: "1", maximum: "60000", on_dirty: on_dirty }
                        NumberField { form, disabled, path: "equipment_source.request_timeout_seconds", label: t!("admin-runtime-source-timeout"), minimum: "1", maximum: "30", on_dirty: on_dirty }
                    }
                    div { class: "admin-runtime-field-grid admin-runtime-field-grid--lists",
                        ListField { form, disabled, path: "equipment_source.allowed_hosts", label: t!("admin-runtime-source-hosts"), on_dirty: on_dirty }
                        ListField { form, disabled, path: "equipment_source.allowed_path_prefixes", label: t!("admin-runtime-source-paths"), on_dirty: on_dirty }
                    }
                }
            }

            RuntimeSection { title: t!("admin-runtime-section-mailboxes"), open: false,
                p { class: "admin-runtime-section-note", {t!("admin-runtime-mailboxes-note")} }
                div { class: "admin-runtime-mailboxes",
                    for (index, mailbox) in mailboxes.into_iter().enumerate() {
                        div { class: "admin-runtime-mailbox", key: "mailbox-{index}",
                            label {
                                span { {t!("admin-runtime-mailbox-world")} }
                                select {
                                    name: "mailboxes-{index}-world",
                                    autocomplete: "off",
                                    disabled,
                                    value: mailbox.world_id,
                                    onchange: {
                                        let mut form = form;
                                        let on_dirty = on_dirty;
                                        move |event: FormEvent| {
                                            form.write().set_mailbox_world(index, event.value());
                                            on_dirty.call(());
                                        }
                                    },
                                    option { value: "main", {t!("admin-runtime-mailbox-main")} }
                                    option { value: "isekai", {t!("admin-runtime-mailbox-isekai")} }
                                }
                            }
                            label {
                                span { {t!("admin-runtime-mailbox-key")} }
                                input {
                                    r#type: "text",
                                    name: "mailboxes-{index}-key",
                                    autocomplete: "off",
                                    autocapitalize: "none",
                                    spellcheck: "false",
                                    disabled,
                                    value: mailbox.mailbox_key,
                                    oninput: {
                                        let mut form = form;
                                        let on_dirty = on_dirty;
                                        move |event: FormEvent| {
                                            form.write().set_mailbox_key(index, event.value());
                                            on_dirty.call(());
                                        }
                                    },
                                }
                            }
                            button {
                                class: "admin-btn-secondary",
                                r#type: "button",
                                disabled,
                                onclick: {
                                    let mut form = form;
                                    let on_dirty = on_dirty;
                                    move |_| {
                                        form.write().remove_mailbox(index);
                                        on_dirty.call(());
                                    }
                                },
                                {t!("admin-runtime-mailbox-remove")}
                            }
                        }
                    }
                    button {
                        class: "admin-btn-secondary admin-runtime-mailbox-add",
                        r#type: "button",
                        disabled: disabled || !can_add_mailbox,
                        onclick: {
                            let mut form = form;
                            let on_dirty = on_dirty;
                            move |_| {
                                form.write().add_mailbox();
                                on_dirty.call(());
                            }
                        },
                        {t!("admin-runtime-mailbox-add")}
                    }
                }
            }

            RuntimeSection { title: t!("admin-runtime-section-templates"), open: false,
                p { class: "admin-runtime-section-note", {t!("admin-runtime-templates-note")} }
                div { class: "admin-runtime-template-grid",
                    TemplateField { form, disabled, path: "mail_templates.auction_assets_rejected", label: t!("admin-runtime-template-auction-assets"), on_dirty: on_dirty }
                    TemplateField { form, disabled, path: "mail_templates.auction_no_attachments", label: t!("admin-runtime-template-no-attachments"), on_dirty: on_dirty }
                    TemplateField { form, disabled, path: "mail_templates.auction_equipment_rejected", label: t!("admin-runtime-template-equipment-rejected"), on_dirty: on_dirty }
                    TemplateField { form, disabled, path: "mail_templates.user_assets_rejected", label: t!("admin-runtime-template-user-assets"), on_dirty: on_dirty }
                    TemplateField { form, disabled, path: "mail_templates.register_key", label: t!("admin-runtime-template-register-key"), on_dirty: on_dirty }
                    TemplateField { form, disabled, path: "mail_templates.register_result", label: t!("admin-runtime-template-register-result"), on_dirty: on_dirty }
                    TemplateField { form, disabled, path: "mail_templates.rebind_result", label: t!("admin-runtime-template-rebind-result"), on_dirty: on_dirty }
                    TemplateField { form, disabled, path: "mail_templates.rekey_key", label: t!("admin-runtime-template-rekey-key"), on_dirty: on_dirty }
                    TemplateField { form, disabled, path: "mail_templates.rekey_result", label: t!("admin-runtime-template-rekey-result"), on_dirty: on_dirty }
                    TemplateField { form, disabled, path: "mail_templates.settlement_winner_equipment_cod", label: t!("admin-runtime-template-winner-cod"), on_dirty: on_dirty }
                    TemplateField { form, disabled, path: "mail_templates.settlement_payment_carrier", label: t!("admin-runtime-template-payment-carrier"), on_dirty: on_dirty }
                    TemplateField { form, disabled, path: "mail_templates.settlement_payment_reminder", label: t!("admin-runtime-template-payment-reminder"), on_dirty: on_dirty }
                    TemplateField { form, disabled, path: "mail_templates.settlement_equipment_handoff", label: t!("admin-runtime-template-equipment-handoff"), on_dirty: on_dirty }
                    TemplateField { form, disabled, path: "mail_templates.settlement_seller_payout", label: t!("admin-runtime-template-seller-payout"), on_dirty: on_dirty }
                    TemplateField { form, disabled, path: "mail_templates.settlement_buyer_refund", label: t!("admin-runtime-template-buyer-refund"), on_dirty: on_dirty }
                    TemplateField { form, disabled, path: "mail_templates.operations_alert", label: t!("admin-runtime-template-operations-alert"), on_dirty: on_dirty }
                }
            }

            RuntimeSection { title: t!("admin-runtime-section-listener"), open: false,
                p { class: "admin-runtime-section-warning", {t!("admin-runtime-listener-warning")} }
                div { class: "admin-runtime-field-grid",
                    SelectField {
                        form,
                        disabled,
                        path: "listener.mode",
                        label: t!("admin-runtime-listener-mode"),
                        options: vec![
                            ("tcp".to_owned(), t!("admin-runtime-listener-tcp")),
                            ("unix".to_owned(), t!("admin-runtime-listener-unix")),
                        ],
                        on_dirty: on_dirty,
                    }
                    if listener_mode == "tcp" {
                        TextField { form, disabled, path: "listener.address", label: t!("admin-runtime-listener-address"), on_dirty: on_dirty }
                    } else {
                        TextField { form, disabled, path: "listener.group", label: t!("admin-runtime-listener-group"), on_dirty: on_dirty }
                        TextField { form, disabled, path: "listener.socket_path", label: t!("admin-runtime-listener-socket"), on_dirty: on_dirty }
                        TextField { form, disabled, path: "listener.runtime_directory", label: t!("admin-runtime-listener-directory"), on_dirty: on_dirty }
                    }
                }
            }

            RuntimeSection { title: t!("admin-runtime-section-security"), open: false,
                p { class: "admin-runtime-section-warning", {t!("admin-runtime-security-warning")} }
                div { class: "admin-runtime-field-grid",
                    SwitchField { form, disabled, path: "security.allowed_origins_enabled", label: t!("admin-runtime-security-origins-enabled"), on_dirty: on_dirty }
                    SwitchField { form, disabled, path: "security.cookie_secure", label: t!("admin-runtime-security-cookie-secure"), on_dirty: on_dirty }
                    SwitchField { form, disabled, path: "security.trusted_unix_proxy", label: t!("admin-runtime-security-trusted-unix"), on_dirty: on_dirty }
                    NumberField { form, disabled, path: "security.login_rate_limit", label: t!("admin-runtime-security-login-limit"), minimum: "1", maximum: "10000", on_dirty: on_dirty }
                    NumberField { form, disabled, path: "security.mutation_rate_limit", label: t!("admin-runtime-security-mutation-limit"), minimum: "1", maximum: "10000", on_dirty: on_dirty }
                    NumberField { form, disabled, path: "security.rate_limit_window_seconds", label: t!("admin-runtime-security-window"), minimum: "1", maximum: "3600", on_dirty: on_dirty }
                    NumberField { form, disabled, path: "security.request_timeout_seconds", label: t!("admin-runtime-security-timeout"), minimum: "1", maximum: "120", on_dirty: on_dirty }
                }
                div { class: "admin-runtime-field-grid admin-runtime-field-grid--lists",
                    ListField { form, disabled, path: "security.allowed_origins", label: t!("admin-runtime-security-origins"), on_dirty: on_dirty }
                    ListField { form, disabled, path: "security.trusted_proxy_cidrs", label: t!("admin-runtime-security-proxy-cidrs"), on_dirty: on_dirty }
                }
                RuntimeGroup { title: t!("admin-runtime-session-title"),
                    div { class: "admin-runtime-field-grid",
                        NumberField { form, disabled, path: "session.idle_ttl_seconds", label: t!("admin-runtime-session-idle"), minimum: "60", maximum: "86400", on_dirty: on_dirty }
                        NumberField { form, disabled, path: "session.absolute_ttl_seconds", label: t!("admin-runtime-session-absolute"), minimum: "60", maximum: "2592000", on_dirty: on_dirty }
                        NumberField { form, disabled, path: "session.max_public_sessions", label: t!("admin-runtime-session-maximum"), minimum: "1", maximum: "100", on_dirty: on_dirty }
                    }
                }
            }

            RuntimeSection { title: t!("admin-runtime-section-observability"), open: false,
                div { class: "admin-runtime-field-grid",
                    SwitchField { form, disabled, path: "metrics.enabled", label: t!("admin-runtime-metrics-enabled"), on_dirty: on_dirty }
                    SwitchField { form, disabled, path: "tracing.enabled", label: t!("admin-runtime-tracing-enabled"), on_dirty: on_dirty }
                    TextField { form, disabled, path: "tracing.endpoint", label: t!("admin-runtime-tracing-endpoint"), on_dirty: on_dirty }
                    NumberField { form, disabled, path: "tracing.batch_timeout_seconds", label: t!("admin-runtime-tracing-batch-timeout"), minimum: "1", maximum: "30", on_dirty: on_dirty }
                    NumberField { form, disabled, path: "tracing.export_timeout_seconds", label: t!("admin-runtime-tracing-export-timeout"), minimum: "1", maximum: "30", on_dirty: on_dirty }
                    NumberField { form, disabled, path: "tracing.max_export_batch_size", label: t!("admin-runtime-tracing-batch-size"), minimum: "1", maximum: "512", on_dirty: on_dirty }
                    NumberField { form, disabled, path: "tracing.queue_capacity", label: t!("admin-runtime-tracing-queue"), minimum: "1", maximum: "4096", on_dirty: on_dirty }
                    NumberField { form, disabled, path: "tracing.sample_ratio_ppm", label: t!("admin-runtime-tracing-sample"), minimum: "0", maximum: "1000000", on_dirty: on_dirty }
                    NumberField { form, disabled, path: "tracing.shutdown_timeout_seconds", label: t!("admin-runtime-tracing-shutdown"), minimum: "1", maximum: "30", on_dirty: on_dirty }
                }
            }

            RuntimeSection { title: t!("admin-runtime-section-worker"), open: false,
                div { class: "admin-runtime-field-grid",
                    TextField { form, disabled, path: "worker_control.socket_path", label: t!("admin-runtime-worker-socket"), on_dirty }
                }
            }
        }
    }
}

#[component]
fn RuntimeSection(title: String, open: bool, children: Element) -> Element {
    rsx! {
        details { class: "admin-runtime-section", open,
            summary { span { "{title}" } }
            div { class: "admin-runtime-section__body", {children} }
        }
    }
}

#[component]
fn RuntimeGroup(title: String, children: Element) -> Element {
    rsx! {
        section { class: "admin-runtime-group",
            h3 { "{title}" }
            {children}
        }
    }
}

#[component]
fn SwitchField(
    mut form: Signal<RuntimeConfigForm>,
    disabled: bool,
    path: &'static str,
    label: String,
    on_dirty: EventHandler<()>,
) -> Element {
    // 整份草稿会随每次输入变化；只让当前字段值的变化触发控件重绘。
    let checked = use_memo(use_reactive((&path,), move |(path,)| form.read().boolean(path)));
    rsx! {
        label { class: "admin-runtime-switch",
            input {
                id: validation::field_id(path),
                name: path,
                "data-runtime-label": label.clone(),
                r#type: "checkbox",
                disabled,
                checked: checked(),
                onchange: move |event: FormEvent| {
                    form.write().set_boolean(path, event.checked());
                    on_dirty.call(());
                },
            }
            span { class: "admin-runtime-switch__track", aria_hidden: "true" }
            span { class: "admin-runtime-switch__label", "{label}" }
        }
    }
}

#[component]
fn TextField(
    mut form: Signal<RuntimeConfigForm>,
    disabled: bool,
    path: &'static str,
    label: String,
    on_dirty: EventHandler<()>,
) -> Element {
    let value = use_memo(use_reactive((&path,), move |(path,)| form.read().input(path)));
    rsx! {
        label { class: "admin-runtime-field",
            span { "{label}"
                if super::hot_fields::is_hot(path) {
                    span { class: "admin-runtime-hot-badge", title: t!("admin-runtime-hot-field-title"), {t!("admin-runtime-hot-field-badge")} }
                }
            }
            input {
                id: validation::field_id(path),
                name: path,
                autocomplete: "off",
                autocapitalize: "none",
                spellcheck: "false",
                "data-runtime-label": label.clone(),
                r#type: "text",
                disabled,
                value: value(),
                oninput: move |event: FormEvent| {
                    form.write().set_input(path, event.value());
                    on_dirty.call(());
                },
            }
        }
    }
}

#[component]
fn NumberField(
    mut form: Signal<RuntimeConfigForm>,
    disabled: bool,
    path: &'static str,
    label: String,
    minimum: &'static str,
    maximum: &'static str,
    on_dirty: EventHandler<()>,
) -> Element {
    let value = use_memo(use_reactive((&path,), move |(path,)| form.read().input(path)));
    let issues = use_context::<FormIssues>();
    let message =
        issues.0.read().iter().find(|issue| issue.path == path).map(|issue| issue.message.clone());
    let error_id = format!("{}-error", validation::field_id(path));
    rsx! {
        label { class: "admin-runtime-field",
            span { "{label}"
                if super::hot_fields::is_hot(path) {
                    span { class: "admin-runtime-hot-badge", title: t!("admin-runtime-hot-field-title"), {t!("admin-runtime-hot-field-badge")} }
                }
            }
            input {
                id: validation::field_id(path),
                name: path,
                autocomplete: "off",
                inputmode: "numeric",
                "data-runtime-number": path,
                "data-runtime-label": label.clone(),
                aria_invalid: message.as_ref().map(|_| "true"),
                aria_describedby: message.as_ref().map(|_| error_id.clone()),
                r#type: "number",
                disabled,
                min: minimum,
                max: maximum,
                step: "1",
                value: value(),
                oninput: move |event: FormEvent| {
                    form.write().set_input(path, event.value());
                    on_dirty.call(());
                },
            }
            if let Some(message) = message.as_ref() {
                small { id: error_id.clone(), class: "admin-config-field-error", "{message}" }
            }
        }
    }
}

#[component]
fn ListField(
    mut form: Signal<RuntimeConfigForm>,
    disabled: bool,
    path: &'static str,
    label: String,
    on_dirty: EventHandler<()>,
) -> Element {
    let value = use_memo(use_reactive((&path,), move |(path,)| form.read().input(path)));
    rsx! {
        label { class: "admin-runtime-field admin-runtime-field--list",
            span { "{label}" }
            textarea {
                id: validation::field_id(path),
                name: path,
                autocomplete: "off",
                autocapitalize: "none",
                spellcheck: "false",
                "data-runtime-label": label.clone(),
                disabled,
                rows: "3",
                value: value(),
                oninput: move |event: FormEvent| {
                    form.write().set_input(path, event.value());
                    on_dirty.call(());
                },
            }
            small { {t!("admin-runtime-list-hint")} }
        }
    }
}

#[component]
fn TemplateField(
    mut form: Signal<RuntimeConfigForm>,
    disabled: bool,
    path: &'static str,
    label: String,
    on_dirty: EventHandler<()>,
) -> Element {
    let value = use_memo(use_reactive((&path,), move |(path,)| form.read().input(path)));
    rsx! {
        label { class: "admin-runtime-field admin-runtime-field--template",
            span { "{label}" }
            textarea {
                id: validation::field_id(path),
                name: path,
                autocomplete: "off",
                "data-runtime-label": label.clone(),
                disabled,
                rows: "4",
                value: value(),
                oninput: move |event: FormEvent| {
                    form.write().set_input(path, event.value());
                    on_dirty.call(());
                },
            }
        }
    }
}

#[component]
fn SelectField(
    mut form: Signal<RuntimeConfigForm>,
    disabled: bool,
    path: &'static str,
    label: String,
    options: Vec<(String, String)>,
    on_dirty: EventHandler<()>,
) -> Element {
    let value = use_memo(use_reactive((&path,), move |(path,)| form.read().input(path)));
    rsx! {
        label { class: "admin-runtime-field",
            span { "{label}" }
            select {
                id: validation::field_id(path),
                name: path,
                autocomplete: "off",
                "data-runtime-label": label.clone(),
                disabled,
                value: value(),
                onchange: move |event: FormEvent| {
                    form.write().set_input(path, event.value());
                    on_dirty.call(());
                },
                for (option_value, option_label) in options {
                    option { value: option_value, "{option_label}" }
                }
            }
        }
    }
}
