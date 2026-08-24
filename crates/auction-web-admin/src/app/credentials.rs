//! Admin 凭据管理页面。
//!
//! 页面只展示凭据安全投影；导入后的 secret 不会回填，也不会出现在列表、错误提示或
//! 浏览器持久化状态中。修改成功后刷新列表，等待 Worker 重新加载运行时凭据。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

use auction_web_shared::TruncatedText;

use crate::api::{
    AdminCredential, AdminCredentialPage, create_admin_credential, delete_admin_credential,
    fetch_admin_credentials, mutate_admin_credential,
};

use super::pagination::AdminPager;
use super::{AdminLoadError, AdminPanel, format_admin_time};

/// 加载并展示 Admin 凭据管理页面。
pub(super) fn page() -> Element {
    let reload_generation = use_signal(|| 0_u64);
    let kind_filter = use_signal(String::new);
    let world_filter = use_signal(String::new);
    let status_filter = use_signal(String::new);
    let page_number = use_signal(|| 1_i64);
    let mut pending_delete = use_signal(|| None::<AdminCredential>);
    let mut delete_busy = use_signal(|| false);
    let mut delete_feedback = use_signal(|| None::<bool>);
    let mut delete_command_state = use_signal(|| None::<(String, Uuid)>);
    let credentials = use_resource(move || {
        let _ = reload_generation();
        fetch_admin_credentials(kind_filter(), world_filter(), status_filter(), page_number())
    });

    let open_delete = move |credential: AdminCredential| {
        if !delete_busy() {
            delete_feedback.set(None);
            pending_delete.set(Some(credential));
        }
    };
    let confirm_delete = move |credential: AdminCredential| {
        if delete_busy() {
            return;
        }
        delete_feedback.set(None);
        delete_busy.set(true);
        let signature = format!("delete:{}:{}", credential.id, credential.state_version);
        let command_id = stable_command_id(delete_command_state, signature);
        let mut delete_busy = delete_busy;
        let mut delete_feedback = delete_feedback;
        let mut pending_delete = pending_delete;
        let mut reload_generation = reload_generation;
        spawn(async move {
            let successful = delete_admin_credential(command_id, &credential).await.is_ok();
            delete_feedback.set(Some(successful));
            delete_busy.set(false);
            if successful {
                pending_delete.set(None);
                delete_command_state.set(None);
                reload_generation.set(reload_generation().saturating_add(1));
            }
        });
    };

    match &*credentials.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-credentials-title"), detail: t!("admin-credentials-loading") }
        },
        Some(Err(_)) => rsx! {
            AdminLoadError {
                title: t!("page-credentials-title"),
                detail: t!("admin-credentials-error"),
                refresh: reload_generation,
            }
        },
        Some(Ok(page)) => {
            let pending = pending_delete();
            rsx! {
                div { class: "admin-credentials",
                    h1 { id: "admin-credentials-title", {t!("page-credentials-title")} }
                    CredentialCreateForm { reload_generation }
                    CredentialFilters { kind_filter, world_filter, status_filter, page_number }
                    CredentialTable {
                        page: page.clone(),
                        reload_generation,
                        on_delete: open_delete,
                    }
                    CredentialPager { page: page.clone(), page_number }
                }
                if let Some(credential) = pending {
                    CredentialDeleteDialog {
                        credential,
                        busy: delete_busy(),
                        on_confirm: confirm_delete,
                        on_cancel: move |_| pending_delete.set(None),
                    }
                }
                if let Some(successful) = delete_feedback() {
                    p {
                        class: if successful {
                            "admin-panel__note action-feedback action-feedback--success"
                        } else {
                            "admin-panel__note action-feedback action-feedback--error"
                        },
                        role: if successful { "status" } else { "alert" },
                        if successful { {t!("admin-credentials-action-success")} } else { {t!("admin-credentials-action-error")} }
                    }
                }
            }
        }
    }
}

#[component]
fn CredentialFilters(
    mut kind_filter: Signal<String>,
    mut world_filter: Signal<String>,
    mut status_filter: Signal<String>,
    mut page_number: Signal<i64>,
) -> Element {
    rsx! {
        section { class: "admin-credentials__filters", aria_labelledby: "admin-credentials-filter-title",
            h2 { id: "admin-credentials-filter-title", {t!("admin-credentials-filter")} }
            div { class: "admin-panel__actions",
                label {
                    {t!("admin-credentials-kind")}
                    select {
                        value: kind_filter(),
                        onchange: move |event| {
                            kind_filter.set(event.value());
                            page_number.set(1);
                        },
                        option { value: "", {t!("admin-credentials-all")} }
                        option { value: "mail_cookie", {t!("admin-credentials-kind-mail")} }
                        option { value: "forum_cookie", {t!("admin-credentials-kind-forum")} }
                    }
                }
                label {
                    {t!("admin-credentials-world")}
                    select {
                        value: world_filter(),
                        onchange: move |event| {
                            world_filter.set(event.value());
                            page_number.set(1);
                        },
                        option { value: "", {t!("admin-credentials-all")} }
                        option { value: "main", {t!("admin-credentials-world-main")} }
                        option { value: "isekai", {t!("admin-credentials-world-isekai")} }
                        option { value: "shared", {t!("admin-credentials-world-shared")} }
                    }
                }
                label {
                    {t!("admin-credentials-status")}
                    select {
                        value: status_filter(),
                        onchange: move |event| {
                            status_filter.set(event.value());
                            page_number.set(1);
                        },
                        option { value: "", {t!("admin-credentials-all")} }
                        option { value: "active", {t!("admin-credentials-status-active")} }
                        option { value: "disabled", {t!("admin-credentials-status-disabled")} }
                        option { value: "invalid", {t!("admin-credentials-status-invalid")} }
                    }
                }
            }
        }
    }
}

#[component]
fn CredentialCreateForm(reload_generation: Signal<u64>) -> Element {
    let mut kind = use_signal(|| "mail_cookie".to_owned());
    let mut world = use_signal(|| "main".to_owned());
    let mut label = use_signal(String::new);
    let mut secret = use_signal(String::new);
    let mut command_id_state = use_signal(|| None::<Uuid>);
    let mut busy = use_signal(|| false);
    let mut feedback = use_signal(|| None::<bool>);

    rsx! {
        section {
            class: "admin-credentials__import",
            aria_labelledby: "admin-credentials-create-title",
            aria_busy: busy().to_string(),
            h2 { id: "admin-credentials-create-title", {t!("admin-credentials-create-title")} }
            p { class: "admin-panel__note", {t!("admin-credentials-secret-note")} }
            form {
                class: "admin-credentials__create-form",
                onsubmit: move |event| {
                    event.prevent_default();
                    if busy() {
                        return;
                    }
                    let kind = kind();
                    let world = world();
                    let label = label();
                    let raw_secret = secret();
                    if label.trim().is_empty() || raw_secret.is_empty() {
                        feedback.set(Some(false));
                        return;
                    }
                    let mut secret_signal = secret;
                    let command_id = match command_id_state() {
                        Some(command_id) => command_id,
                        None => {
                            let command_id = Uuid::new_v4();
                            command_id_state.set(Some(command_id));
                            command_id
                        }
                    };
                    busy.set(true);
                    feedback.set(None);
                    let mut busy = busy;
                    let mut feedback = feedback;
                    let mut reload_generation = reload_generation;
                    spawn(async move {
                        let result =
                            create_admin_credential(command_id, kind, world, label, raw_secret).await;
                        match result {
                            Ok(_) => {
                                secret_signal.set(String::new());
                                command_id_state.set(None);
                                feedback.set(Some(true));
                                reload_generation.set(reload_generation().saturating_add(1));
                            }
                            Err(_) => feedback.set(Some(false)),
                        }
                        busy.set(false);
                    });
                },
                label {
                    r#for: "admin-credentials-kind",
                    {t!("admin-credentials-kind")}
                    select {
                        id: "admin-credentials-kind",
                        disabled: busy(),
                        value: kind(),
                        onchange: move |event| {
                            kind.set(event.value());
                            command_id_state.set(None);
                        },
                        option { value: "mail_cookie", {t!("admin-credentials-kind-mail")} }
                        option { value: "forum_cookie", {t!("admin-credentials-kind-forum")} }
                    }
                }
                label {
                    r#for: "admin-credentials-world",
                    {t!("admin-credentials-world")}
                    select {
                        id: "admin-credentials-world",
                        disabled: busy(),
                        value: world(),
                        onchange: move |event| {
                            world.set(event.value());
                            command_id_state.set(None);
                        },
                        option { value: "main", {t!("admin-credentials-world-main")} }
                        option { value: "isekai", {t!("admin-credentials-world-isekai")} }
                        option { value: "shared", {t!("admin-credentials-world-shared")} }
                    }
                }
                label {
                    r#for: "admin-credentials-label",
                    {t!("admin-credentials-label")}
                    input {
                        id: "admin-credentials-label",
                        disabled: busy(),
                        required: true,
                        value: label(),
                        oninput: move |event| {
                            label.set(event.value());
                            command_id_state.set(None);
                        },
                    }
                }
                label {
                    r#for: "admin-credentials-secret",
                    {t!("admin-credentials-secret")}
                    input {
                        id: "admin-credentials-secret",
                        r#type: "password",
                        autocomplete: "new-password",
                        disabled: busy(),
                        required: true,
                        value: secret(),
                        oninput: move |event| {
                            secret.set(event.value());
                            command_id_state.set(None);
                        },
                    }
                }
                button {
                    r#type: "submit",
                    disabled: busy() || label().trim().is_empty() || secret().is_empty(),
                    if busy() { {t!("admin-action-working")} } else { {t!("admin-credentials-create-submit")} }
                }
            }
            if let Some(success) = feedback() {
                p {
                    class: if success {
                        "admin-panel__note action-feedback action-feedback--success"
                    } else {
                        "admin-panel__note action-feedback action-feedback--error"
                    },
                    role: if success { "status" } else { "alert" },
                    if success { {t!("admin-credentials-action-success")} } else { {t!("admin-credentials-action-error")} }
                }
            }
        }
    }
}

#[component]
fn CredentialTable(
    page: AdminCredentialPage,
    reload_generation: Signal<u64>,
    on_delete: EventHandler<AdminCredential>,
) -> Element {
    rsx! {
        section {
            class: "admin-credentials__list",
            aria_labelledby: "admin-credentials-title",
            p { class: "admin-panel__note", {t!("admin-credentials-summary", total: page.total)} }
            if page.restart_required {
                p { class: "admin-panel__note", {t!("admin-credentials-restart-note")} }
            }
            if page.credentials.is_empty() {
                p { class: "admin-panel__note", {t!("admin-credentials-empty")} }
            } else {
                div { class: "public-auction-catalog admin-credentials__table-wrap",
                    table {
                        thead {
                            tr {
                                th { {t!("admin-credentials-kind")} }
                                th { {t!("admin-credentials-world")} }
                                th { {t!("admin-credentials-label")} }
                                th { {t!("admin-credentials-status")} }
                                th { {t!("admin-credentials-rotated-at")} }
                                th { {t!("admin-credentials-version")} }
                                th { {t!("admin-credentials-actions")} }
                            }
                        }
                        tbody {
                            for credential in page.credentials {
                                CredentialRow { credential, reload_generation, on_delete }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn CredentialRow(
    credential: AdminCredential,
    reload_generation: Signal<u64>,
    on_delete: EventHandler<AdminCredential>,
) -> Element {
    let mut rename_value = use_signal(|| credential.label.clone());
    let mut rename_dialog = use_signal(|| false);
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<bool>);
    let command_state = use_signal(|| None::<(String, Uuid)>);
    let credential_for_rename = credential.clone();
    let credential_for_rename_submit = credential.clone();
    let credential_for_status = credential.clone();
    let credential_for_delete = credential.clone();
    let next_status = match credential.status.as_str() {
        "active" => Some("disabled"),
        "disabled" => Some("active"),
        _ => None,
    };
    rsx! {
        tr { class: "admin-credentials__row",
            td { "data-label": t!("admin-credentials-kind"), {credential_kind_label(&credential.kind)} }
            td { "data-label": t!("admin-credentials-world"), TruncatedText { value: credential_world_label(&credential.world), size: "compact" } }
            td { "data-label": t!("admin-credentials-label"), TruncatedText { value: credential.label.clone() } }
            td { "data-label": t!("admin-credentials-status"),
                span {
                    class: credential_status_class(&credential.status),
                    {credential_status_label(&credential.status)}
                }
            }
            td { "data-label": t!("admin-credentials-rotated-at"),
                if let Some(rotated_at) = credential.rotated_at.clone() {
                    time { datetime: rotated_at.clone(), "{format_admin_time(&rotated_at)}" }
                } else {
                    {t!("value-empty")}
                }
            }
            td {
                "data-label": t!("admin-credentials-version"),
                class: "public-auction-catalog__number",
                "{credential.state_version}"
            }
            td { "data-label": t!("admin-credentials-actions"), class: "admin-credentials__row-actions",
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| {
                        rename_value.set(credential_for_rename.label.clone());
                        rename_dialog.set(true);
                    },
                    {t!("admin-credentials-action-rename-open")}
                }
                if let Some(status) = next_status {
                    button {
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| {
                            let credential = credential_for_status.clone();
                            let command_id = stable_command_id(
                                command_state,
                                format!("set_status:{}:{status}", credential.state_version),
                            );
                            run_action(
                                busy,
                                feedback,
                                reload_generation,
                                command_state,
                                async move {
                                    mutate_admin_credential(
                                        command_id,
                                        &credential,
                                        "set_status",
                                        None,
                                        Some(status.to_owned()),
                                    )
                                    .await
                                    .map(|_| ())
                                },
                            );
                        },
                        {credential_status_action_label(&credential.status)}
                    }
                }
                button {
                    r#type: "button",
                    class: "admin-btn-danger",
                    disabled: busy(),
                    onclick: move |_| on_delete.call(credential_for_delete.clone()),
                    {t!("admin-credentials-action-delete")}
                }
                if let Some(success) = feedback() {
                    span {
                        class: if success {
                            "admin-credentials__feedback admin-credentials__feedback--success"
                        } else {
                            "admin-credentials__feedback admin-credentials__feedback--error"
                        },
                        if success { {t!("admin-credentials-action-success")} } else { {t!("admin-credentials-action-error")} }
                    }
                }
            }
        }
        if rename_dialog() {
            tr { class: "admin-credentials__dialog-row",
                td { colspan: "7",
                    div { class: "admin-credentials__dialog-backdrop", role: "presentation",
                        section {
                            class: "admin-credentials__dialog",
                            role: "dialog",
                            aria_modal: "true",
                            aria_labelledby: "admin-credentials-rename-title",
                            aria_busy: busy().to_string(),
                            tabindex: "-1",
                            onkeydown: move |event| {
                                if event.key() == Key::Escape && !busy() {
                                    rename_dialog.set(false);
                                }
                            },
                            h2 { id: "admin-credentials-rename-title", {t!("admin-credentials-rename-title")} }
                            form {
                                onsubmit: move |event| {
                                    event.prevent_default();
                                    if busy() {
                                        return;
                                    }
                                    let credential = credential_for_rename_submit.clone();
                                    let label = rename_value();
                                    let command_id = stable_command_id(
                                        command_state,
                                        format!("rename:{}:{label}", credential.state_version),
                                    );
                                    let mut rename_dialog = rename_dialog;
                                    run_action(
                                        busy,
                                        feedback,
                                        reload_generation,
                                        command_state,
                                        async move {
                                            let result = mutate_admin_credential(
                                                command_id,
                                                &credential,
                                                "rename",
                                                Some(label),
                                                None,
                                            )
                                            .await
                                            .map(|_| ());
                                            if result.is_ok() {
                                                rename_dialog.set(false);
                                            }
                                            result
                                        },
                                    );
                                },
                                label {
                                    {t!("admin-credentials-label")}
                                    input {
                                        r#type: "text",
                                        autofocus: true,
                                        required: true,
                                        disabled: busy(),
                                        value: rename_value(),
                                        oninput: move |event| rename_value.set(event.value()),
                                    }
                                }
                                div { class: "admin-credentials__dialog-actions",
                                    button {
                                        r#type: "button",
                                        class: "admin-btn-secondary",
                                        disabled: busy(),
                                        onclick: move |_| rename_dialog.set(false),
                                        {t!("admin-credentials-cancel")}
                                    }
                                    button {
                                        r#type: "submit",
                                        class: "admin-btn-primary",
                                        disabled: busy(),
                                        if busy() { {t!("admin-action-working")} } else { {t!("admin-credentials-rename-save")} }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn CredentialPager(page: AdminCredentialPage, page_number: Signal<i64>) -> Element {
    rsx! {
        AdminPager {
            page: page.page,
            page_size: page.page_size,
            total: page.total,
            label: t!("admin-credentials-pagination"),
            on_page_change: move |next_page| page_number.set(next_page),
        }
    }
}

#[component]
fn CredentialDeleteDialog(
    credential: AdminCredential,
    busy: bool,
    on_confirm: EventHandler<AdminCredential>,
    on_cancel: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "admin-credentials__dialog-backdrop", role: "presentation",
            section {
                class: "admin-credentials__dialog",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "admin-credentials-delete-title",
                aria_busy: busy.to_string(),
                tabindex: "-1",
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !busy {
                        on_cancel.call(());
                    }
                },
                h2 { id: "admin-credentials-delete-title", {t!("admin-credentials-delete-title")} }
                p { {t!("admin-credentials-delete-warning", label: credential.label.clone())} }
                div { class: "admin-credentials__dialog-actions",
                    button {
                        r#type: "button",
                        class: "admin-btn-secondary",
                        disabled: busy,
                        autofocus: true,
                        onclick: move |_| on_cancel.call(()),
                        {t!("admin-credentials-cancel")}
                    }
                    button {
                        r#type: "button",
                        class: "admin-btn-danger",
                        disabled: busy,
                        onclick: move |_| on_confirm.call(credential.clone()),
                        if busy { {t!("admin-action-working")} } else { {t!("admin-credentials-action-delete")} }
                    }
                }
            }
        }
    }
}

fn run_action<F>(
    mut busy: Signal<bool>,
    mut feedback: Signal<Option<bool>>,
    mut reload_generation: Signal<u64>,
    mut command_state: Signal<Option<(String, Uuid)>>,
    action: F,
) where
    F: std::future::Future<Output = Result<(), String>> + 'static,
{
    busy.set(true);
    feedback.set(None);
    spawn(async move {
        let success = action.await.is_ok();
        feedback.set(Some(success));
        if success {
            command_state.set(None);
            reload_generation.set(reload_generation().saturating_add(1));
        }
        busy.set(false);
    });
}

fn stable_command_id(mut command_state: Signal<Option<(String, Uuid)>>, signature: String) -> Uuid {
    match command_state() {
        Some((current, command_id)) if current == signature => command_id,
        _ => {
            let command_id = Uuid::new_v4();
            command_state.set(Some((signature, command_id)));
            command_id
        }
    }
}

fn credential_kind_label(kind: &str) -> String {
    match kind {
        "mail_cookie" => t!("admin-credentials-kind-mail"),
        "forum_cookie" => t!("admin-credentials-kind-forum"),
        _ => kind.to_owned(),
    }
}

fn credential_world_label(world: &str) -> String {
    match world {
        "main" => t!("admin-credentials-world-main"),
        "isekai" => t!("admin-credentials-world-isekai"),
        "shared" => t!("admin-credentials-world-shared"),
        _ => world.to_owned(),
    }
}

fn credential_status_label(status: &str) -> String {
    match status {
        "active" => t!("admin-credentials-status-active"),
        "disabled" => t!("admin-credentials-status-disabled"),
        "invalid" => t!("admin-credentials-status-invalid"),
        _ => status.to_owned(),
    }
}

fn credential_status_class(status: &str) -> &'static str {
    match status {
        "active" => "admin-credentials__status admin-credentials__status--active",
        "disabled" => "admin-credentials__status admin-credentials__status--disabled",
        "invalid" => "admin-credentials__status admin-credentials__status--invalid",
        _ => "admin-credentials__status",
    }
}

fn credential_status_action_label(status: &str) -> String {
    match status {
        "active" => t!("admin-credentials-action-disable"),
        "disabled" => t!("admin-credentials-action-enable"),
        _ => t!("admin-credentials-action-status"),
    }
}
