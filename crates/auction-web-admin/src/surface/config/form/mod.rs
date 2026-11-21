//! 运行时配置的分组表单控件。

mod equipment_colors;
mod number;
mod operations;

use auction_web_shared::t;
use dioxus::prelude::*;

use self::number::NumberInput;
use super::field_announcement::{FieldLiveRegion, use_field_announcer};
use super::model::{MAX_DASHBOARD_MAILBOXES, RuntimeConfigForm};
use super::validation;
use crate::surface::admin_world_label;

#[component]
pub(super) fn RuntimeConfigFields(
    form: Signal<RuntimeConfigForm>,
    disabled: bool,
    on_dirty: EventHandler<()>,
) -> Element {
    let announcer = use_field_announcer();
    use_context_provider(|| announcer);
    let listener_mode = form.read().input("listener.mode");
    let mailboxes = form.read().mailboxes();
    let can_add_mailbox = mailboxes.len() < MAX_DASHBOARD_MAILBOXES;
    let too_many_mailboxes = mailboxes.len() > MAX_DASHBOARD_MAILBOXES;

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
                    RuntimeGroup { title: admin_world_label("main"),
                        SwitchField { form, disabled, path: "worlds.main.enabled", label: t!("admin-runtime-world-enabled"), on_dirty: on_dirty }
                        SwitchField { form, disabled, path: "worlds.main.sender_allowlist_disabled", label: t!("admin-runtime-world-all-senders"), on_dirty: on_dirty }
                        ListField { form, disabled, path: "worlds.main.sender_allowlist", label: t!("admin-runtime-world-senders"), on_dirty: on_dirty }
                    }
                    RuntimeGroup { title: admin_world_label("isekai"),
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
                if too_many_mailboxes {
                    p { class: "admin-config-field-error", id: "admin-runtime-mailbox-count-error",
                        {validation::mailbox_count_error()}
                    }
                }
                div { class: "admin-runtime-mailboxes",
                    id: validation::field_id("dashboard_mailboxes"),
                    tabindex: "-1",
                    role: "group",
                    aria_label: t!("admin-runtime-section-mailboxes"),
                    aria_describedby: too_many_mailboxes.then_some("admin-runtime-mailbox-count-error"),
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
                                    option { value: "main", {admin_world_label("main")} }
                                    option { value: "isekai", {admin_world_label("isekai")} }
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
        FieldLiveRegion { announcer }
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
                "data-runtime-label": label,
                r#type: "checkbox",
                disabled,
                checked: checked(),
                onchange: move |event: FormEvent| {
                    form.write().set_boolean(path, event.checked());
                    on_dirty.call(());
                },
            }
            span { class: "admin-runtime-switch__track", aria_hidden: "true" }
            span { class: "admin-runtime-switch__label",
                "{label}"
                if super::hot_fields::is_hot(path) {
                    span { class: "admin-runtime-hot-badge", title: t!("admin-runtime-hot-field-title"), {t!("admin-runtime-hot-field-badge")} }
                }
            }
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
                "data-runtime-label": label,
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
    let input = use_memo(use_reactive((&path,), move |(path,)| {
        let form = form.read();
        // 由控件报告原始值的范围错误，未编辑的候选字段仍保留原值。
        let original = form.source_input(path).map(str::to_owned);
        (form.input(path), original)
    }));
    let (value, original) = input.cloned();
    rsx! {
        NumberInput {
            path: path.to_owned(), label, minimum, maximum, original, value, disabled,
            on_input: move |value| {
                form.write().set_input(path, value);
                on_dirty.call(());
            },
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
                "data-runtime-label": label,
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
fn SelectField(
    mut form: Signal<RuntimeConfigForm>,
    disabled: bool,
    path: &'static str,
    label: String,
    options: Vec<(String, String)>,
    on_dirty: EventHandler<()>,
) -> Element {
    let value = use_memo(use_reactive((&path,), move |(path,)| form.read().input(path)));
    // 没有匹配项时沿用首项占位，候选文档中的值由操作者显式更改。
    let selected = value.with(|current| {
        options.iter().position(|(option_value, _)| option_value == current).unwrap_or(0)
    });
    rsx! {
        label { class: "admin-runtime-field",
            span { "{label}" }
            select {
                id: validation::field_id(path),
                name: path,
                autocomplete: "off",
                "data-runtime-label": label,
                disabled,
                onchange: move |event: FormEvent| {
                    form.write().set_input(path, event.value());
                    on_dirty.call(());
                },
                for (index, (option_value, option_label)) in options.into_iter().enumerate() {
                    option {
                        key: "{option_value}",
                        value: "{option_value}",
                        selected: index == selected,
                        "{option_label}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use auction_contracts::{AdminRuntimeConfigResponse, ApiResponse};
    use auction_web_shared::i18n::{DEFAULT_ADMIN_LOCALE, build_bundle};
    use dioxus::core::{AttributeValue, ElementId, Mutation, ScopeId};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::{i18n_bundle, try_use_init_i18n_from_bundle};
    use serde_json::json;
    use unic_langid::langid;

    use super::{RuntimeConfigFields, SelectField};
    use crate::api::fetch_admin_runtime_config;
    use crate::surface::ADMIN_LOCALE_CATALOG;
    use crate::surface::config::model::RuntimeConfigForm;
    #[cfg(not(target_arch = "wasm32"))]
    use crate::surface::config::test_form::{dispatch, listener};
    use crate::surface::config::validation::{self, FormIssues};
    #[cfg(not(target_arch = "wasm32"))]
    use crate::surface::config::validation::{NumberControl, NumberDocument};
    use crate::surface::realtime::selected_options;

    fn projection() -> ApiResponse<AdminRuntimeConfigResponse> {
        serde_json::from_str(include_str!(
            "../../../../../auction-contracts/tests/fixtures/admin/runtime-config-default.json"
        ))
        .unwrap()
    }

    fn read_form(response: ApiResponse<AdminRuntimeConfigResponse>) -> RuntimeConfigForm {
        let dom = VirtualDom::prebuilt(VNode::empty).with_root_context(response);
        let decoded = dom.in_scope(ScopeId::APP, || {
            let mut fetch = pin!(fetch_admin_runtime_config());
            let Poll::Ready(response) =
                fetch.as_mut().poll(&mut Context::from_waker(Waker::noop()))
            else {
                panic!("夹具读取应在当前轮完成");
            };
            response.unwrap()
        });
        RuntimeConfigForm::from_response(&decoded).unwrap()
    }

    #[component]
    fn EditorFields(model: RuntimeConfigForm) -> Element {
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_ADMIN_LOCALE,
                ADMIN_LOCALE_CATALOG.default_ftl,
                &DEFAULT_ADMIN_LOCALE,
                None,
            )
        })
        .unwrap();
        let form = use_signal(|| model);
        let mut issues = use_signal(Vec::new);
        use_context_provider(|| FormIssues(issues));
        rsx! {
            form {
                novalidate: true,
                onsubmit: move |event| {
                    event.prevent_default();
                    issues.set(validation::collect_issues(&form.read()));
                },
                if !issues.read().is_empty() {
                    validation::ErrorSummary { issues: issues() }
                }
                RuntimeConfigFields { form, disabled: false, on_dirty: |_| {} }
            }
        }
    }

    fn field(edits: &[Mutation], path: &str) -> ElementId {
        let field_id = validation::field_id(path);
        edits
            .iter()
            .find_map(|edit| match edit {
                Mutation::SetAttribute {
                    id,
                    name: "id",
                    value: AttributeValue::Text(value),
                    ..
                } if value == &field_id => Some(*id),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn config_selects_show_unix_and_thursday_on_mount() {
        let mut response = projection();
        response.data.document["listener"]["mode"] = json!("unix");
        response.data.document["listener"]["address"] = json!("");
        response.data.document["listener"]["socket_path"] = json!("/run/hv-auction/admin.sock");
        response.data.document["operations"]["schedule"]["managed"] = json!(true);
        let model = read_form(response);
        let mut dom = VirtualDom::new_with_props(EditorFields, EditorFieldsProps { model });
        let edits = dom.rebuild_to_vec().edits;
        let selected = selected_options(&edits);
        assert!(selected.contains(&"unix"));
        assert!(selected.contains(&"Thursday"));
        assert!(!selected.contains(&"tcp"));
        assert!(!selected.contains(&"Monday"));
    }

    #[test]
    fn missing_select_option_uses_first_without_rewriting_the_candidate() {
        let mut response = projection();
        response.data.document["listener"]["mode"] = json!("unix");
        let model = read_form(response);
        let (page, edits) = crate::surface::auctions::editor::tests::mount_writer(move || {
            let form = use_signal(|| model.clone());
            use_context_provider(|| form);
            rsx! {
                SelectField { form, disabled: false, path: "listener.mode", label: "监听器模式",
                    options: vec![("tcp".to_owned(), "TCP".to_owned())], on_dirty: |_| {} }
            }
        });
        assert_eq!(selected_options(&edits), ["tcp"]);
        assert_eq!(
            page.dom().in_scope(ScopeId::APP, || consume_context::<Signal<RuntimeConfigForm>>()
                .read()
                .to_document()
                .unwrap()["listener"]["mode"]
                .clone()),
            "unix"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn submitted_color_level_summary_retranslates_without_resubmitting() {
        let mut response = projection();
        response.data.document["equipment_colors"]["rules"] = json!([color_rule(0)]);
        let model = read_form(response);
        let mut dom = VirtualDom::new_with_props(EditorFields, EditorFieldsProps { model });
        let initial = dom.rebuild_to_vec().edits;
        let path = &validation::color_level_path(0, "min");
        let input = field(&initial, path);
        dispatch(&dom, input, "input", "1001");
        let typing = dom.render_immediate_to_vec().edits;
        assert!(!has_attribute(&typing, input, "aria-invalid", "true"));
        dispatch(&dom, listener(&initial, "submit"), "submit", "");
        let submitted = dom.render_immediate_to_vec().edits;
        assert!(submitted.iter().any(|edit| matches!(edit, Mutation::CreateTextNode { value, .. }
            if value.contains("规则 1 / 最低等级") && value.contains("请输入 0 至 1000 之间的整数"))));

        switch_to_japanese(&dom);
        let switched = dom.render_immediate_to_vec().edits;
        assert!(switched.iter().any(|edit| matches!(edit, Mutation::SetText { value, .. }
            if value.contains("ルール 1 / 最小レベル") && value.contains("0 から 1000 までの整数"))));
        assert!(!has_attribute(&switched, input, "aria-invalid", "false"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn switch_to_japanese(dom: &VirtualDom) {
        dom.in_scope(ScopeId::APP, || {
            let japanese = build_bundle(
                &DEFAULT_ADMIN_LOCALE,
                ADMIN_LOCALE_CATALOG.default_ftl,
                &langid!("ja"),
                Some(include_str!("../../../../i18n/ja.ftl")),
            )
            .unwrap();
            i18n_bundle().set_bundle(japanese);
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn static_number_summary_starts_with_the_displayed_label_and_retranslates() {
        let model = read_form(projection());
        let browser = NumberDocument::default();
        let mut dom = VirtualDom::new_with_props(EditorFields, EditorFieldsProps { model })
            .with_root_context(browser.clone());
        let initial = dom.rebuild_to_vec().edits;
        let path = "equipment_filter.level.min";
        let input = field(&initial, path);
        let attr = |name| {
            crate::surface::realtime::text_attributes(&initial, name)
                .find_map(|(id, value)| (id == input).then_some(value))
                .unwrap()
                .to_owned()
        };
        let label = format!("装备收录 / 装备过滤 / {}", attr("data-runtime-label"));
        browser.0.borrow_mut().push(NumberControl {
            path: path.to_owned(),
            minimum: attr("min"),
            maximum: attr("max"),
            label,
        });
        dispatch(&dom, input, "input", "1001");
        let typing = dom.render_immediate_to_vec().edits;
        assert!(!has_attribute(&typing, input, "aria-invalid", "true"));
        dispatch(&dom, listener(&initial, "submit"), "submit", "");
        let submitted = dom.render_immediate_to_vec().edits;
        assert!(submitted.iter().any(|edit| matches!(edit, Mutation::CreateTextNode { value, .. }
            if value.contains("装备收录 / 装备过滤 / 最低等级") && value.contains("请输入 0 至 1000 之间的整数"))));
        assert!(
            !submitted.iter().any(|edit| matches!(edit, Mutation::CreateTextNode { value, .. }
            if value.contains(path)))
        );

        browser.0.borrow_mut()[0].label = "装備受付 / 装備フィルター / 最小レベル".to_owned();
        switch_to_japanese(&dom);
        let switched: Vec<_> = (0..4).flat_map(|_| dom.render_immediate_to_vec().edits).collect();
        assert!(switched.iter().any(|edit| matches!(edit, Mutation::SetText { value, .. }
            if value.contains("装備受付 / 装備フィルター / 最小レベル") && value.contains("0 から 1000 までの整数"))));
    }

    fn has_attribute(edits: &[Mutation], field: ElementId, name: &str, expected: &str) -> bool {
        edits.iter().any(|edit| matches!(edit,
            Mutation::SetAttribute { id, name: attribute, value: AttributeValue::Text(value), .. }
                if *id == field && *attribute == name && value == expected
        ))
    }

    #[test]
    fn negative_threshold_is_rendered_with_a_field_error_and_round_trips_unedited() {
        let mut response = projection();
        let path = "operations.alerts.delivery_pending_count_threshold";
        response.data.document["operations"]["alerts"]["managed"] = json!(true);
        response.data.document["operations"]["alerts"]["delivery_pending_count_threshold"] =
            json!(-1);
        let mut model = read_form(response.clone());

        let mut dom =
            VirtualDom::new_with_props(EditorFields, EditorFieldsProps { model: model.clone() });
        let edits = dom.rebuild_to_vec().edits;
        let input = field(&edits, path);
        assert!(has_attribute(&edits, input, "value", "-1"));
        assert!(has_attribute(&edits, input, "aria-invalid", "true"));
        assert!(has_attribute(
            &edits,
            input,
            "aria-describedby",
            &format!("{}-error", validation::field_id(path))
        ));
        assert!(edits.iter().any(|edit| matches!(edit,
            Mutation::CreateTextNode { value, .. } if value.contains("之间的整数"))));

        assert_eq!(model.edited_input(path), None);
        let mut expected = response.data.document;
        let metrics = !expected["metrics"]["enabled"].as_bool().unwrap();
        model.set_boolean("metrics.enabled", metrics);
        expected["metrics"]["enabled"] = json!(metrics);
        assert_eq!(model.to_document().unwrap(), expected);

        model.set_input(path, "12".to_owned());
        assert_eq!(
            model.to_document().unwrap()["operations"]["alerts"]["delivery_pending_count_threshold"],
            12
        );
    }

    #[test]
    fn nonnegative_historical_number_outside_the_range_is_marked_and_preserved() {
        let mut response = projection();
        let path = "tracing.queue_capacity";
        response.data.document["tracing"]["queue_capacity"] = json!(0);
        let mut model = read_form(response.clone());
        let mut dom =
            VirtualDom::new_with_props(EditorFields, EditorFieldsProps { model: model.clone() });
        let edits = dom.rebuild_to_vec().edits;
        let input = field(&edits, path);
        assert!(has_attribute(&edits, input, "value", "0"));
        assert!(has_attribute(&edits, input, "aria-invalid", "true"));
        let error_id = format!("{}-error", validation::field_id(path));
        assert!(has_attribute(&edits, input, "aria-describedby", &error_id));
        assert!(edits.iter().any(|edit| matches!(edit,
            Mutation::CreateTextNode { value, .. } if value.contains("之间的整数")
        )));

        assert_eq!(model.edited_input(path), None);
        let mut expected = response.data.document;
        let metrics = !expected["metrics"]["enabled"].as_bool().unwrap();
        model.set_boolean("metrics.enabled", metrics);
        expected["metrics"]["enabled"] = json!(metrics);
        assert_eq!(model.to_document().unwrap(), expected);
    }

    fn color_rule(level_min: i32) -> serde_json::Value {
        json!({
            "category": "Cloth", "equipment_type": "Phase", "quality": "Legendary",
            "level": {"min": level_min, "max": 500}, "prefixes": ["Radiant"], "suffixes": [],
            "prefix_color": "#ff0000", "suffix_color": "#00aa00", "combined_color": "#aa00aa",
        })
    }

    #[test]
    fn conflicting_color_selections_and_extra_mailboxes_open_with_repair_controls() {
        let mut response = projection();
        let mut rule = color_rule(0);
        rule["categories"] = json!(["Light"]);
        response.data.document["equipment_colors"]["rules"] = json!([rule]);
        response.data.document["dashboard_mailboxes"] = json!([
            {"world_id": "main", "mailbox_key": "mail-1"},
            {"world_id": "isekai", "mailbox_key": "mail-2"},
            {"world_id": "main", "mailbox_key": "mail-3"},
        ]);
        let model = read_form(response.clone());
        assert_eq!(model.to_document().unwrap(), response.data.document);
        let mut dom = VirtualDom::new_with_props(EditorFields, EditorFieldsProps { model });
        let edits = dom.rebuild_to_vec().edits;
        for expected in
            ["旧单值和新数组同时存在", "保留旧单值", "保留新数组", "仪表盘邮箱超过允许的"]
        {
            assert!(
                edits.iter().any(|edit| matches!(edit,
                    Mutation::CreateTextNode { value, .. } if value.contains(expected)
                )),
                "{expected}"
            );
        }
        assert!(edits.iter().any(|edit| matches!(edit,
            Mutation::SetAttribute { name: "value", value: AttributeValue::Text(value), .. }
                if value == "mail-3"
        )));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn clearing_an_edited_level_waits_for_blur_or_submit_and_valid_input_clears_the_error() {
        for trigger in ["blur", "submit"] {
            let mut response = projection();
            response.data.document["equipment_colors"]["rules"] = json!([color_rule(-1)]);
            let model = read_form(response);
            let mut dom = VirtualDom::new_with_props(EditorFields, EditorFieldsProps { model });
            let edits = dom.rebuild_to_vec().edits;
            let input = field(&edits, &validation::color_level_path(0, "min"));
            let submit = listener(&edits, "submit");
            assert!(has_attribute(&edits, input, "value", "-1"));
            assert!(has_attribute(&edits, input, "aria-invalid", "true"));

            dispatch(&dom, input, "input", "5");
            assert!(has_attribute(
                &dom.render_immediate_to_vec().edits,
                input,
                "aria-invalid",
                "false"
            ));
            dispatch(&dom, input, "input", "");
            let typing = dom.render_immediate_to_vec().edits;
            assert!(!has_attribute(&typing, input, "aria-invalid", "true"));
            dispatch(&dom, if trigger == "submit" { submit } else { input }, trigger, "");
            assert!(has_attribute(
                &dom.render_immediate_to_vec().edits,
                input,
                "aria-invalid",
                "true"
            ));

            dispatch(&dom, input, "input", "-2");
            assert!(!has_attribute(
                &dom.render_immediate_to_vec().edits,
                input,
                "aria-invalid",
                "false"
            ));
            dispatch(&dom, input, "input", "7");
            assert!(has_attribute(
                &dom.render_immediate_to_vec().edits,
                input,
                "aria-invalid",
                "false"
            ));
        }
    }
}
