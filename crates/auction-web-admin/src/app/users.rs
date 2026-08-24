//! Admin 用户管理页面。
//!
//! 页面只维护筛选、表单和安全用户投影；所有改变状态的动作都由后端命令 API 生成
//! `command_id`，页面会在同一输入重试时复用它，并使用返回的 `state_version`。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

use auction_web_shared::TruncatedText;

use crate::api::{
    AdminUser, AdminUserStatusFilter, blacklist_admin_user, create_admin_user, fetch_admin_users,
    mutate_admin_user, unblacklist_admin_user,
};

use super::pagination::AdminPager;
use super::{AdminLoadError, format_admin_time};

/// 加载并展示 Admin 用户管理页面。
pub(super) fn page() -> Element {
    let name_filter = use_signal(String::new);
    let name_draft = use_signal(String::new);
    let status_filter = use_signal(|| AdminUserStatusFilter::All);
    let page_number = use_signal(|| 1_i64);
    let reload_generation = use_signal(|| 0_u64);
    let mut create_open = use_signal(|| false);
    let mut generated_key = use_signal(|| None::<AdminUserGeneratedKey>);
    let users = use_resource(move || {
        let _ = reload_generation();
        fetch_admin_users(name_filter(), status_filter(), page_number())
    });

    match &*users.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-users-title"), detail: t!("admin-users-loading") }
        },
        Some(Err(_)) => rsx! {
            AdminLoadError {
                title: t!("page-users-title"),
                detail: t!("admin-users-error"),
                refresh: reload_generation,
            }
        },
        Some(Ok(page)) => rsx! {
            div { class: "admin-users",
                header { class: "admin-users__heading",
                    h1 { id: "admin-users-title", {t!("page-users-title")} }
                    button {
                        class: "admin-users__create-opener admin-btn-primary",
                        r#type: "button",
                        onclick: move |_| create_open.set(true),
                        {t!("admin-users-create-open")}
                    }
                }
                if create_open() {
                    AdminUserCreateForm {
                        reload_generation,
                        on_created: move |key| {
                            generated_key.set(key);
                            create_open.set(false);
                        },
                        on_close: move |_| create_open.set(false),
                    }
                }
                AdminUserTable {
                    page: page.clone(),
                    name_filter,
                    name_draft,
                    status_filter,
                    page_number,
                    reload_generation,
                    on_generated_key: move |key| generated_key.set(Some(key)),
                }
            }
            if let Some(key) = generated_key() {
                AdminUserGeneratedKeyDialog {
                    generated: key,
                    on_close: move |_| generated_key.set(None),
                }
            }
        },
    }
}

#[derive(Debug, Clone, PartialEq)]
struct AdminUserGeneratedKey {
    username: String,
    key: String,
}

#[component]
fn AdminUserCreateForm(
    reload_generation: Signal<u64>,
    on_created: EventHandler<Option<AdminUserGeneratedKey>>,
    on_close: EventHandler<()>,
) -> Element {
    let mut username = use_signal(String::new);
    let mut game_player_id = use_signal(String::new);
    let mut key_mode = use_signal(|| "generated".to_owned());
    let mut manual_key = use_signal(String::new);
    let mut command_id_state = use_signal(|| None::<Uuid>);
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<bool>);

    rsx! {
        div { class: "admin-users__dialog-backdrop", role: "presentation",
            section {
                class: "admin-users__dialog",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "admin-users-create-title",
                aria_busy: busy().to_string(),
                tabindex: "-1",
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !busy() {
                        on_close.call(());
                    }
                },
                h2 { id: "admin-users-create-title", {t!("admin-users-create-title")} }
                form {
                    class: "admin-panel__actions admin-users__create-grid",
                    onsubmit: move |event| {
                        event.prevent_default();
                        if busy() {
                            return;
                        }
                        let username = username();
                        let game_player_id = game_player_id();
                        let key_mode = key_mode();
                        let manual_key = manual_key();
                        if username.trim().is_empty()
                            || game_player_id.trim().is_empty()
                            || (key_mode == "manual" && manual_key.is_empty())
                        {
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
                        let mut busy = busy;
                        let mut feedback = feedback;
                        let mut reload_generation = reload_generation;
                        let on_created = on_created;
                        busy.set(true);
                        spawn(async move {
                            let result = create_admin_user(
                                command_id,
                                username,
                                game_player_id,
                                if key_mode == "manual" { "manual" } else { "generated" },
                                (key_mode == "manual").then_some(manual_key),
                            )
                            .await;
                            match result {
                                Ok(result) => {
                                    let generated_key = result.generated_key.map(|key| {
                                        AdminUserGeneratedKey {
                                            username: result.user.username,
                                            key,
                                        }
                                    });
                                    on_created.call(generated_key);
                                    feedback.set(Some(true));
                                    command_id_state.set(None);
                                    reload_generation.set(reload_generation().saturating_add(1));
                                }
                                Err(_) => feedback.set(Some(false)),
                            }
                            busy.set(false);
                        });
                    },
                    label {
                        r#for: "admin-users-username",
                        {t!("admin-users-username")}
                        input {
                            id: "admin-users-username",
                            r#type: "text",
                            required: true,
                            autofocus: true,
                            disabled: busy(),
                            value: username(),
                            oninput: move |event| {
                                username.set(event.value());
                                command_id_state.set(None);
                            },
                        }
                    }
                    label {
                        r#for: "admin-users-game-player-id",
                        {t!("admin-users-game-player-id")}
                        input {
                            id: "admin-users-game-player-id",
                            r#type: "text",
                            required: true,
                            disabled: busy(),
                            value: game_player_id(),
                            oninput: move |event| {
                                game_player_id.set(event.value());
                                command_id_state.set(None);
                            },
                        }
                    }
                    label {
                        r#for: "admin-users-key-mode",
                        {t!("admin-users-key-mode")}
                        select {
                            id: "admin-users-key-mode",
                            disabled: busy(),
                            value: key_mode(),
                            onchange: move |event| {
                                key_mode.set(event.value());
                                command_id_state.set(None);
                            },
                            option { value: "generated", {t!("admin-users-key-generated")} }
                            option { value: "manual", {t!("admin-users-key-manual")} }
                        }
                    }
                    if key_mode() == "manual" {
                        label {
                            r#for: "admin-users-manual-key",
                            {t!("admin-users-manual-key")}
                            input {
                                id: "admin-users-manual-key",
                                r#type: "password",
                                autocomplete: "new-password",
                                required: true,
                                disabled: busy(),
                                value: manual_key(),
                                oninput: move |event| {
                                    manual_key.set(event.value());
                                    command_id_state.set(None);
                                },
                            }
                        }
                    }
                    div { class: "admin-users__dialog-actions",
                        button {
                            r#type: "button",
                            class: "admin-btn-secondary",
                            disabled: busy(),
                            onclick: move |_| on_close.call(()),
                            {t!("admin-users-create-close")}
                        }
                        button {
                            r#type: "submit",
                            class: "admin-btn-primary",
                            disabled: busy(),
                            if busy() { {t!("admin-action-working")} } else { {t!("admin-users-create-submit")} }
                        }
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
                        if success { {t!("admin-users-created")} } else { {t!("admin-users-action-error")} }
                    }
                }
            }
        }
    }
}

#[component]
fn AdminUserGeneratedKeyDialog(
    generated: AdminUserGeneratedKey,
    on_close: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "admin-users__dialog-backdrop", role: "presentation",
            section {
                class: "admin-users__dialog admin-users__secret-dialog",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "admin-users-generated-key-title",
                tabindex: "-1",
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        on_close.call(());
                    }
                },
                h2 { id: "admin-users-generated-key-title", {t!("admin-users-generated-key-title")} }
                p { {t!("admin-users-generated-key-warning", username: generated.username)} }
                code { {generated.key} }
                div { class: "admin-users__dialog-actions",
                    button {
                        r#type: "button",
                        class: "admin-btn-primary",
                        autofocus: true,
                        onclick: move |_| on_close.call(()),
                        {t!("admin-users-generated-key-dismiss")}
                    }
                }
            }
        }
    }
}

#[component]
fn AdminUserTable(
    page: crate::api::AdminUserPage,
    mut name_filter: Signal<String>,
    mut name_draft: Signal<String>,
    mut status_filter: Signal<AdminUserStatusFilter>,
    mut page_number: Signal<i64>,
    reload_generation: Signal<u64>,
    on_generated_key: EventHandler<AdminUserGeneratedKey>,
) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-users-title",
            p { class: "admin-panel__note", {t!("admin-users-summary", total: page.pagination.total)} }
            form {
                class: "admin-panel__actions",
                onsubmit: move |event| {
                    event.prevent_default();
                    name_filter.set(name_draft().trim().to_owned());
                    page_number.set(1);
                },
                label {
                    {t!("admin-users-filter-name")}
                    input {
                        value: name_draft(),
                        oninput: move |event| {
                            name_draft.set(event.value());
                        },
                    }
                }
                label {
                    {t!("admin-users-filter-status")}
                    select {
                        value: status_filter_value(status_filter()),
                        onchange: move |event| {
                            status_filter.set(parse_status_filter(&event.value()));
                            page_number.set(1);
                        },
                        option { value: "all", {t!("admin-users-status-all")} }
                        option { value: "active", {t!("admin-users-status-active")} }
                        option { value: "blacklisted", {t!("admin-users-status-blacklisted")} }
                    }
                }
            }
            if page.users.is_empty() {
                p { class: "admin-panel__note", role: "status", {t!("admin-users-no-results")} }
            } else {
                div { class: "public-auction-catalog admin-users__table-wrap",
                    table {
                        thead { tr {
                            th { {t!("admin-users-username")} }
                            th { {t!("admin-users-game-player-id")} }
                            th { {t!("admin-users-status")} }
                            th { {t!("admin-users-blacklist-history")} }
                            th { {t!("admin-users-blacklist-count")} }
                            th { {t!("admin-users-created-at")} }
                            th { {t!("admin-users-key-status")} }
                            th { {t!("admin-users-version")} }
                            th { {t!("admin-users-actions")} }
                        } }
                        tbody {
                            for user in page.users {
                                AdminUserRow { user, reload_generation, on_generated_key }
                            }
                        }
                    }
                }
            }
            AdminPager {
                page: page.pagination.page,
                page_size: page.pagination.page_size,
                total: page.pagination.total,
                label: t!("page-users-title"),
                on_page_change: move |next_page| page_number.set(next_page),
            }
        }
    }
}

#[component]
fn AdminUserRow(
    user: AdminUser,
    reload_generation: Signal<u64>,
    on_generated_key: EventHandler<AdminUserGeneratedKey>,
) -> Element {
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<String>);
    let mut rename_value = use_signal(|| user.username.clone());
    let mut game_player_id_value = use_signal(|| user.game_player_id.clone().unwrap_or_default());
    let mut dialog = use_signal(|| None::<AdminUserDialogKind>);
    let mut dialog_reason = use_signal(String::new);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    let user_for_rename = user.clone();
    let user_for_game_dialog = user.clone();
    let user_for_revoke = user.clone();
    let user_for_reissue = user.clone();
    let user_for_blacklist = user.clone();
    let user_for_action = user.clone();
    let dialog_title = dialog().map(|kind| kind.title(&user.username));
    rsx! {
        tr { class: "admin-users__row",
            td { "data-label": t!("admin-users-username"), TruncatedText { value: user.username.clone() } }
            td { "data-label": t!("admin-users-game-player-id"), TruncatedText { value: user.game_player_id.clone().unwrap_or_else(|| t!("admin-users-empty")), size: "wide" } }
            td { "data-label": t!("admin-users-status"),
                span {
                    class: user_status_class(&user.status),
                    {user_status_label(&user.status)}
                }
            }
            td { "data-label": t!("admin-users-blacklist-history"), class: "admin-users__history",
                if user.blacklist_history.is_empty() {
                    span { class: "admin-users__history-empty", {t!("admin-users-blacklist-never")} }
                } else {
                    details {
                        summary {
                            {user.blacklist_history.first().map(|entry| entry.public_reason.clone()).unwrap_or_default()}
                        }
                        ol {
                            for entry in user.blacklist_history.iter() {
                                li {
                                    span { {entry.public_reason.clone()} }
                                    time { datetime: entry.blacklisted_at.clone(), "{format_admin_time(&entry.blacklisted_at)}" }
                                }
                            }
                        }
                        if user.blacklist_count > user.blacklist_history.len() as i64 {
                            p { {t!("admin-users-history-more", count: user.blacklist_count - user.blacklist_history.len() as i64)} }
                        }
                    }
                }
            }
            td {
                "data-label": t!("admin-users-blacklist-count"),
                class: "public-auction-catalog__number",
                "{user.blacklist_count}"
            }
            td { "data-label": t!("admin-users-created-at"),
                time { datetime: user.created_at.clone(), "{format_admin_time(&user.created_at)}" }
            }
            td { "data-label": t!("admin-users-key-status"),
                span {
                    class: user_key_status_class(&user.key_status),
                    {user_key_status_label(&user.key_status)}
                }
            }
            td { "data-label": t!("admin-users-version"), "{user.state_version}" }
            td { "data-label": t!("admin-users-actions"), class: "admin-users__row-actions",
                input {
                    value: rename_value(),
                    aria_label: t!("admin-users-rename-input"),
                            oninput: move |event| {
                                rename_value.set(event.value());
                                command_state.set(None);
                            },
                }
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| {
                        let user = user_for_rename.clone();
                        let next_username = rename_value();
                        let command_id = stable_command_id(
                            command_state,
                            format!("rename:{}:{next_username}", user.state_version),
                        );
                        run_user_action(
                            busy,
                            feedback,
                            reload_generation,
                            command_state,
                            on_generated_key,
                            user.username.clone(),
                            async move {
                            mutate_admin_user(command_id, &user, "rename", Some(next_username), None)
                                .await
                                .map(|_| None)
                            },
                        );
                    },
                    {t!("admin-users-action-touch-rename")}
                }
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| dialog.set(Some(AdminUserDialogKind::GamePlayerId)),
                    {t!("admin-users-action-set-game-player-id")}
                }
                button {
                    r#type: "button",
                    disabled: busy() || user.key_status == "revoked",
                    onclick: move |_| dialog.set(Some(AdminUserDialogKind::RevokeKey)),
                    {t!("admin-users-action-revoke-key")}
                }
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| dialog.set(Some(AdminUserDialogKind::ReissueKey)),
                    {t!("admin-users-action-reissue-key")}
                }
                if user.status == "blacklisted" {
                    button {
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| dialog.set(Some(AdminUserDialogKind::Unblacklist)),
                        {t!("admin-users-action-unblacklist")}
                    }
                } else {
                    button {
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| {
                            dialog_reason.set(t!("admin-users-default-reason"));
                            dialog.set(Some(AdminUserDialogKind::Blacklist));
                        },
                        {t!("admin-users-action-blacklist")}
                    }
                }
                if let Some(message) = feedback() {
                    span { class: "admin-users__feedback", {message} }
                }
                if let Some(kind) = dialog() {
                    div { class: "admin-settlement__dialog-backdrop", role: "presentation",
                        section {
                            class: "admin-settlement__dialog admin-users-dialog",
                            role: "dialog",
                            aria_modal: "true",
                            aria_labelledby: "admin-users-dialog-title",
                            aria_busy: busy().to_string(),
                            tabindex: "-1",
                            onkeydown: move |event| {
                                if event.key() == Key::Escape && !busy() {
                                    dialog.set(None);
                                }
                            },
                            h2 { id: "admin-users-dialog-title", {dialog_title.clone().unwrap_or_default()} }
                            if kind == AdminUserDialogKind::GamePlayerId {
                                form {
                                    onsubmit: move |event| {
                                        event.prevent_default();
                                        if busy() {
                                            return;
                                        }
                                        let user = user_for_game_dialog.clone();
                                        let value = game_player_id_value();
                                        let command_id = stable_command_id(
                                            command_state,
                                            format!("set_game_player_id:{}:{value}", user.state_version),
                                        );
                                        let mut dialog = dialog;
                                        run_user_action(
                                            busy,
                                            feedback,
                                            reload_generation,
                                            command_state,
                                            on_generated_key,
                                            user.username.clone(),
                                            async move {
                                            let result = mutate_admin_user(
                                                command_id,
                                                &user,
                                                "set_game_player_id",
                                                None,
                                                Some(value),
                                            )
                                            .await;
                                            if result.is_ok() {
                                                dialog.set(None);
                                            }
                                            result.map(|_| None)
                                            },
                                        );
                                    },
                                    label {
                                        {t!("admin-users-game-player-id")}
                                        input {
                                            r#type: "text",
                                            autofocus: true,
                                            required: true,
                                            disabled: busy(),
                                            value: game_player_id_value(),
                                            oninput: move |event| game_player_id_value.set(event.value()),
                                        }
                                    }
                                    AdminUserDialogActions {
                                        busy,
                                        on_cancel: move |_| dialog.set(None),
                                    }
                                }
                            } else if kind == AdminUserDialogKind::Blacklist {
                                form {
                                    onsubmit: move |event| {
                                        event.prevent_default();
                                        if busy() || dialog_reason().trim().is_empty() {
                                            return;
                                        }
                                        let user = user_for_blacklist.clone();
                                        let reason = dialog_reason();
                                        let command_id = stable_command_id(
                                            command_state,
                                            format!("blacklist:{}:{reason}", user.state_version),
                                        );
                                        let mut dialog = dialog;
                                        run_user_action(
                                            busy,
                                            feedback,
                                            reload_generation,
                                            command_state,
                                            on_generated_key,
                                            user.username.clone(),
                                            async move {
                                            let result = blacklist_admin_user(command_id, &user, reason).await;
                                            if result.is_ok() {
                                                dialog.set(None);
                                            }
                                            result.map(|_| None)
                                            },
                                        );
                                    },
                                    label {
                                        {t!("admin-users-blacklist-reason")}
                                        textarea {
                                            autofocus: true,
                                            required: true,
                                            disabled: busy(),
                                            maxlength: "512",
                                            value: dialog_reason(),
                                            oninput: move |event| dialog_reason.set(event.value()),
                                        }
                                    }
                                    AdminUserDialogActions {
                                        busy,
                                        on_cancel: move |_| dialog.set(None),
                                    }
                                }
                            } else {
                                p { {t!("admin-users-confirm-description")} }
                                div { class: "admin-panel__actions",
                                    button {
                                        r#type: "button",
                                        class: "admin-btn-secondary",
                                        disabled: busy(),
                                        autofocus: true,
                                        onclick: move |_| dialog.set(None),
                                        {t!("admin-users-action-cancel")}
                                    }
                                    button {
                                        r#type: "button",
                                        class: if kind == AdminUserDialogKind::Unblacklist || kind == AdminUserDialogKind::ReissueKey {
                                            "admin-btn-primary"
                                        } else {
                                            "admin-btn-danger"
                                        },
                                        disabled: busy(),
                                        onclick: move |_| {
                                            let user = match kind {
                                                AdminUserDialogKind::RevokeKey => user_for_revoke.clone(),
                                                AdminUserDialogKind::ReissueKey => user_for_reissue.clone(),
                                                AdminUserDialogKind::Unblacklist => user_for_action.clone(),
                                                _ => return,
                                            };
                                            let action = match kind {
                                                AdminUserDialogKind::RevokeKey => "revoke_key",
                                                AdminUserDialogKind::ReissueKey => "reissue_key",
                                                AdminUserDialogKind::Unblacklist => "unblacklist",
                                                _ => return,
                                            };
                                            let command_id = stable_command_id(
                                                command_state,
                                                format!("{action}:{}", user.state_version),
                                            );
                                            let mut dialog = dialog;
                                            run_user_action(
                                                busy,
                                                feedback,
                                                reload_generation,
                                                command_state,
                                                on_generated_key,
                                                user.username.clone(),
                                                async move {
                                                let result = match kind {
                                                    AdminUserDialogKind::RevokeKey => {
                                                        mutate_admin_user(command_id, &user, "revoke_key", None, None)
                                                            .await
                                                            .map(|result| result.generated_key)
                                                    }
                                                    AdminUserDialogKind::ReissueKey => {
                                                        mutate_admin_user(command_id, &user, "reissue_key", None, None)
                                                            .await
                                                            .map(|result| result.generated_key)
                                                    }
                                                    AdminUserDialogKind::Unblacklist => {
                                                        unblacklist_admin_user(command_id, &user).await.map(|_| None)
                                                    }
                                                    _ => Ok(None),
                                                };
                                                if result.is_ok() {
                                                    dialog.set(None);
                                                }
                                                result
                                                },
                                            );
                                        },
                                        if busy() { {t!("admin-action-working")} } else { {t!("admin-users-action-confirm")} }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdminUserDialogKind {
    GamePlayerId,
    ReissueKey,
    RevokeKey,
    Blacklist,
    Unblacklist,
}

impl AdminUserDialogKind {
    fn title(self, username: &str) -> String {
        match self {
            Self::GamePlayerId => t!("admin-users-game-dialog-title", username: username),
            Self::ReissueKey => t!("admin-users-reissue-dialog-title", username: username),
            Self::RevokeKey => t!("admin-users-revoke-dialog-title", username: username),
            Self::Blacklist => t!("admin-users-blacklist-dialog-title", username: username),
            Self::Unblacklist => t!("admin-users-unblacklist-dialog-title", username: username),
        }
    }
}

#[component]
fn AdminUserDialogActions(busy: Signal<bool>, on_cancel: EventHandler<MouseEvent>) -> Element {
    rsx! {
        div { class: "admin-panel__actions",
            button {
                r#type: "button",
                class: "admin-btn-secondary",
                disabled: busy(),
                onclick: on_cancel,
                {t!("admin-users-action-cancel")}
            }
            button {
                r#type: "submit",
                class: "admin-btn-primary",
                disabled: busy(),
                if busy() { {t!("admin-action-working")} } else { {t!("admin-users-action-confirm")} }
            }
        }
    }
}

fn run_user_action<F>(
    mut busy: Signal<bool>,
    mut feedback: Signal<Option<String>>,
    mut reload_generation: Signal<u64>,
    mut command_state: Signal<Option<(String, Uuid)>>,
    on_generated_key: EventHandler<AdminUserGeneratedKey>,
    username: String,
    operation: F,
) where
    F: std::future::Future<Output = Result<Option<String>, String>> + 'static,
{
    busy.set(true);
    spawn(async move {
        let result = operation.await;
        let successful = result.is_ok();
        feedback.set(match result {
            Ok(Some(key)) => {
                on_generated_key.call(AdminUserGeneratedKey { username, key });
                Some(t!("admin-users-updated"))
            }
            Ok(None) => Some(t!("admin-users-updated")),
            Err(_) => Some(t!("admin-users-action-error")),
        });
        if successful {
            command_state.set(None);
        }
        reload_generation.set(reload_generation().saturating_add(1));
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

fn status_filter_value(value: AdminUserStatusFilter) -> &'static str {
    match value {
        AdminUserStatusFilter::All => "all",
        AdminUserStatusFilter::Active => "active",
        AdminUserStatusFilter::Blacklisted => "blacklisted",
    }
}

fn parse_status_filter(value: &str) -> AdminUserStatusFilter {
    match value {
        "active" => AdminUserStatusFilter::Active,
        "blacklisted" => AdminUserStatusFilter::Blacklisted,
        _ => AdminUserStatusFilter::All,
    }
}

fn user_status_label(value: &str) -> String {
    match value {
        "active" => t!("admin-users-status-active"),
        "blacklisted" => t!("admin-users-status-blacklisted"),
        _ => value.to_owned(),
    }
}

fn user_status_class(value: &str) -> &'static str {
    match value {
        "active" => "admin-users__status admin-users__status--active",
        "blacklisted" => "admin-users__status admin-users__status--blacklisted",
        _ => "admin-users__status",
    }
}

fn user_key_status_label(value: &str) -> String {
    match value {
        "active" => t!("admin-users-key-active"),
        "revoked" => t!("admin-users-key-revoked"),
        _ => value.to_owned(),
    }
}

fn user_key_status_class(value: &str) -> &'static str {
    match value {
        "active" => "admin-users__key-status admin-users__key-status--active",
        "revoked" => "admin-users__key-status admin-users__key-status--revoked",
        _ => "admin-users__key-status",
    }
}

use super::AdminPanel;
