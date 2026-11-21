//! 凭据列表、状态操作和改名对话框入口。

use std::cmp::Ordering;

use auction_web_shared::a11y::focus_element_by_id;
use auction_web_shared::{AsyncState, AsyncStateKind, Time, TruncatedText};
use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;

use super::field_validation::{
    CredentialField, CredentialFieldError, CredentialFieldErrors, credential_field_issue_message,
    use_credential_fields,
};
use super::labels::CredentialFeedback;
use super::{
    CredentialCheckAction, credential_kind_label, credential_status_class, credential_status_label,
    credential_world_label,
};
use crate::api::{AdminCredential, AdminCredentialPage, mutate_admin_credential};
use crate::surface::config::{FieldLiveRegion, use_field_announcer};
use crate::surface::pending::{CommandDelivery, CommandReceipt, PendingCommands, set_if_alive};
use crate::surface::sort::{MobileSortControl, SortDirection, SortableTh, sort_rows};

#[component]
pub fn CredentialTable(
    page: AdminCredentialPage,
    reload_generation: Signal<u64>,
    loaded_generation: u64,
    /// 当前排序列（空串 = 服务端原始顺序）与方向；变更经 `on_sort_change` 由页面写回 URL。
    sort: String,
    direction: SortDirection,
    on_sort_change: EventHandler<(String, SortDirection)>,
    on_delete: EventHandler<AdminCredential>,
) -> Element {
    let is_empty = page.credentials.is_empty();
    let mut credentials = page.credentials;
    sort_credentials(&mut credentials, &sort, direction);
    rsx! {
        p { class: "admin-credentials__feedback", {t!("admin-credentials-check-hint")} }
        MobileSortControl {
            columns: vec![
                ("label", t!("admin-credentials-label")),
                ("world", t!("admin-credentials-world")),
                ("kind", t!("admin-credentials-kind")),
                ("status", t!("admin-credentials-status")),
                ("checked", t!("admin-credentials-rotated-at")),
            ],
            sort: sort.clone(),
            direction,
            on_sort_change,
        }
        div { class: "admin-credentials-table-wrap",
            table { class: "admin-credentials-table",
                aria_label: t!("page-credentials-title"),
                // 列序：名称、世界、类型、状态、轮换时间、使用时间、导入者、操作；五个操作按钮允许换行。
                auction_web_shared::TableColumns { layout: "admin-credentials", widths: vec![14, 6, 10, 6, 14, 14, 6, 30] }
                thead {
                    tr {
                        SortableTh {
                            column: "label",
                            label: t!("admin-credentials-label"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                        SortableTh {
                            column: "world",
                            label: t!("admin-credentials-world"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                        SortableTh {
                            column: "kind",
                            label: t!("admin-credentials-kind"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                        SortableTh {
                            column: "status",
                            label: t!("admin-credentials-status"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                        SortableTh {
                            column: "checked",
                            // 保留旧 URL 排序键 checked，但 rotated_at 不能冒充真实检查时间。
                            label: t!("admin-credentials-rotated-at"),
                            active: sort,
                            direction,
                            on_sort: on_sort_change,
                        }
                        th { scope: "col", {t!("admin-credentials-last-used")} }
                        th { scope: "col", {t!("admin-credentials-imported-by")} }
                        th { scope: "col", {t!("admin-credentials-actions")} }
                    }
                }
                tbody {
                    for credential in credentials {
                        CredentialRow {
                            key: "{credential.id}",
                            credential,
                            reload_generation,
                            loaded_generation,
                            on_delete,
                        }
                    }
                }
            }
        }
        if is_empty {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("admin-credentials-empty"),
                detail: "",
            }
        }
    }
}

#[component]
pub fn CredentialRow(
    credential: AdminCredential,
    on_delete: EventHandler<AdminCredential>,
    reload_generation: Signal<u64>,
    loaded_generation: u64,
) -> Element {
    let mut rename_dialog = use_signal(|| false);
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<CredentialFeedback>);
    let owner = use_context::<PendingCommands>();
    // 重读期间的旧行只供展示，写操作必须等待新凭据版本。
    let read_is_current = move || reload_generation() == loaded_generation;
    let blocked = !read_is_current() || owner.contains(&format!("credential:{}", credential.id));
    let credential_for_rename_submit = credential.clone();
    let credential_for_enable = credential.clone();
    let credential_for_disable = credential.clone();
    let credential_for_delete = credential.clone();
    let feedback_id = format!("admin-credential-feedback-{}", credential.id);
    let rename_input_id = format!("label-{}", credential.id);
    rsx! {
        tr { class: "admin-credentials__row",
            td { "data-label": t!("admin-credentials-label"),
                TruncatedText { value: credential.label.clone() }
                if let Some(player_name) = credential.player_name.clone() {
                    span { title: t!("admin-credentials-player-name-help"),
                        {t!("admin-credentials-player-name")}
                        TruncatedText { value: player_name, size: "compact" }
                    }
                }
            }
            td { "data-label": t!("admin-credentials-world"), TruncatedText { value: credential_world_label(&credential.world), size: "compact" } }
            td { "data-label": t!("admin-credentials-kind"), {credential_kind_label(&credential.kind)} }
            td { "data-label": t!("admin-credentials-status"),
                span {
                    class: credential_status_class(&credential.status),
                    {credential_status_label(&credential.status)}
                }
            }
            td { "data-label": t!("admin-credentials-rotated-at"),
                if let Some(rotated_at) = credential.rotated_at.clone() {
                    Time { value: rotated_at }
                } else {
                    {t!("value-empty")}
                }
            }
            td { "data-label": t!("admin-credentials-last-used"), {t!("value-empty")} }
            // 安全 DTO 不含导入者，当前登录 actor 不能代替历史操作事实。
            td { "data-label": t!("admin-credentials-imported-by"), {t!("value-empty")} }
            td {
                "data-label": t!("admin-credentials-actions"),
                aria_describedby: feedback().is_some().then_some(feedback_id),
                div { class: "admin-credentials__row-actions",
                    button {
                        r#type: "button",
                        disabled: busy() || blocked || credential.status == "active",
                        onclick: move |_| {
                            let credential = credential_for_enable.clone();
                            if busy() || !read_is_current() { return; }
                            submit_credential(owner, credential, "set_status", None, Some("active".to_owned()), feedback);
                        },
                        {t!("admin-credentials-action-enable")}
                    }
                    button {
                        r#type: "button",
                        disabled: busy() || blocked || credential.status == "disabled",
                        onclick: move |_| {
                            let credential = credential_for_disable.clone();
                            if busy() || !read_is_current() { return; }
                            submit_credential(owner, credential, "set_status", None, Some("disabled".to_owned()), feedback);
                        },
                        {t!("admin-credentials-action-disable")}
                    }
                    button {
                        r#type: "button",
                        disabled: busy() || blocked,
                        onclick: move |_| {
                            if read_is_current() { rename_dialog.set(true); }
                        },
                        {t!("admin-credentials-action-rename-open")}
                    }
                    button {
                        r#type: "button",
                        class: "admin-btn-danger",
                        disabled: busy() || blocked,
                        onclick: move |_| on_delete.call(credential_for_delete.clone()),
                        {t!("admin-credentials-action-delete")}
                    }
                    CredentialCheckAction { credential: credential.clone(), busy, blocked }
                    if let Some(feedback) = feedback() {
                        span {
                            id: feedback_id.clone(),
                            class: "admin-credentials__feedback admin-credentials__feedback--error",
                            role: "alert",
                            aria_label: format!(
                                "{}: {}",
                                credential.label,
                                feedback.message(),
                            ),
                            {feedback.message()}
                        }
                    }
                }
            }
        }
        if rename_dialog() {
            tr { class: "admin-credentials-dialog-row",
                td { colspan: "8",
                    CredentialRenameDialog {
                        initial_label: credential.label.clone(),
                        input_id: rename_input_id,
                        busy: busy(),
                        blocked,
                        on_close: move |_| rename_dialog.set(false),
                        on_save: move |label| {
                            if !read_is_current() { return; }
                            let credential = credential_for_rename_submit.clone();
                            if submit_credential(owner, credential, "rename", Some(label), None, feedback) {
                                rename_dialog.set(false);
                            }
                        },
                    }
                }
            }
        }
    }
}

#[component]
fn CredentialRenameDialog(
    initial_label: String,
    input_id: String,
    busy: bool,
    #[props(default)] blocked: bool,
    on_close: EventHandler<()>,
    on_save: EventHandler<String>,
) -> Element {
    let label_field = CredentialField::Label;
    let mut rename_value = use_signal(|| initial_label);
    let announcer = use_field_announcer();
    let fields = use_credential_fields(announcer);
    let issue = fields.error(label_field);
    let focus_id = input_id.clone();
    let error_id = format!("{input_id}-error");
    rsx! {
        Dialog {
            title: t!("admin-credentials-rename-title"),
            close_label: t!("admin-credentials-cancel"),
            busy,
            on_close,
            form {
                novalidate: true,
                onsubmit: move |event| {
                    event.prevent_default();
                    if busy || blocked {
                        return;
                    }
                    let label = rename_value();
                    let issue = label_field.issue(&label);
                    let mut errors = CredentialFieldErrors::default();
                    errors.set(label_field, issue);
                    fields.show(errors);
                    if let Some(issue) = issue {
                        focus_element_by_id(&focus_id);
                        announcer.announce(credential_field_issue_message(issue));
                        return;
                    }
                    on_save.call(label);
                },
                label {
                    r#for: input_id.clone(),
                    {t!("admin-credentials-label")}
                    input {
                        id: input_id,
                        r#type: "text",
                        autofocus: true,
                        required: true,
                        disabled: busy,
                        aria_invalid: issue.is_some().to_string(),
                        aria_describedby: issue.is_some().then_some(error_id),
                        value: rename_value(),
                        oninput: move |event| {
                            let value = event.value();
                            fields.edited(label_field, &value);
                            rename_value.set(value);
                        },
                        onblur: move |_| fields.blurred(label_field, &rename_value.peek()),
                    }
                }
                CredentialFieldError { id: error_id.clone(), issue }
                div { class: "admin-shell__dialog-actions",
                    button {
                        r#type: "button",
                        class: "admin-btn-secondary",
                        disabled: busy,
                        onclick: move |_| on_close.call(()),
                        {t!("admin-credentials-cancel")}
                    }
                    button {
                        r#type: "submit",
                        class: "admin-btn-primary",
                        disabled: busy || blocked,
                        if busy { {t!("admin-action-working")} } else { {t!("admin-credentials-rename-save")} }
                    }
                }
            }
            FieldLiveRegion { announcer }
        }
    }
}

/// 只把当前已提交的安全凭据投影与候选值交给页宿主，不持有 Cookie 或结果。
fn submit_credential(
    owner: PendingCommands,
    credential: AdminCredential,
    action: &'static str,
    label: Option<String>,
    status: Option<String>,
    feedback: Signal<Option<CredentialFeedback>>,
) -> bool {
    let action_label = if action == "rename" {
        t!("admin-credentials-action-rename-open")
    } else if status.as_deref() == Some("active") {
        t!("admin-credentials-action-enable")
    } else {
        t!("admin-credentials-action-disable")
    };
    let accepted = owner.submit_with_receipt(
        format!("credential:{}", credential.id),
        action_label,
        move |command_id| {
            let credential = credential.clone();
            let label = label.clone();
            let status = status.clone();
            async move {
                #[cfg(all(test, not(target_arch = "wasm32")))]
                if let Some(reply) = try_consume_context::<tests::CredentialReply>() {
                    return reply
                        .0
                        .fetch()
                        .await
                        .map_err(crate::api::AdminWriteFailure::outcome_unknown);
                }
                mutate_admin_credential(command_id, &credential, action, label, status).await
            }
        },
        move |result, source_alive| {
            if source_alive {
                set_if_alive(feedback, None);
            }
            // 写入后的列表重读可能卸载这一行，重启提示必须随独立的命令回执保留。
            let message = CredentialFeedback::Success { restart_required: result.restart_required };
            CommandDelivery::Receipt(Some(CommandReceipt::Text(message.message())))
        },
        move |code| set_if_alive(feedback, Some(CredentialFeedback::Error(code))),
    );
    if accepted {
        set_if_alive(feedback, None);
    }
    accepted
}
/// 对当前页凭据行排序；`column` 为空或不在允许集合时保持服务端原始顺序。
/// 轮换时间缺省（None）在升序时排在最前；checked 保留为旧 URL 排序键。
fn sort_credentials(credentials: &mut [AdminCredential], column: &str, direction: SortDirection) {
    sort_rows(credentials, column, direction, |left, right, column| match column {
        "label" => left.label.cmp(&right.label),
        "world" => left.world.cmp(&right.world),
        "kind" => left.kind.cmp(&right.kind),
        "status" => left.status.cmp(&right.status),
        "checked" => left.rotated_at.cmp(&right.rotated_at),
        _ => Ordering::Equal,
    });
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
pub(super) mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use auction_contracts::AdminCredentialMutationResponse;
    use auction_web_shared::i18n::{DEFAULT_ADMIN_LOCALE, build_bundle};
    use dioxus::core::{AttributeValue, Mutation};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;
    use gloo_net::http::Method;

    use super::{CredentialRenameDialog, CredentialRow, submit_credential};
    use crate::api::{AdminApiFailure, AdminCredential, AdminCredentialPage};
    use crate::surface::ADMIN_LOCALE_CATALOG;
    use crate::surface::auctions::editor::tests::{
        RecordRead, button, click_button, command_button, has_live_text, init_i18n, mount_writer,
        settle_writer,
    };
    use crate::surface::config::test_form::{dispatch, listener};
    use crate::surface::credentials::labels::CredentialFeedback;
    use crate::surface::credentials::page::page;
    use crate::surface::pagination::{use_admin_pagination, use_versioned_admin_page};
    use crate::surface::pending::{
        PendingCommandHost, PendingCommandNotices, PendingCommands, use_pending_commands,
    };
    use crate::surface::realtime::{submit_forms, text_attributes};

    #[derive(Clone)]
    pub(in crate::surface::credentials) struct CredentialReply(
        pub(in crate::surface::credentials) Rc<RecordRead<AdminCredentialMutationResponse>>,
    );

    pub(in crate::surface::credentials) fn credential() -> AdminCredential {
        AdminCredential {
            id: "0f8fad5b-d9cb-469f-a165-70867728950e".to_owned(),
            kind: "mail_cookie".to_owned(),
            world: "main".to_owned(),
            label: "测试凭据".to_owned(),
            player_name: None,
            status: "active".to_owned(),
            state_version: 1,
            created_at: "2026-11-18T00:00:00Z".to_owned(),
            rotated_at: None,
            updated_at: "2026-11-18T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn credential_restart_receipt_survives_the_failed_list_read() {
        let reads = Rc::new(RecordRead::<AdminCredentialPage>::new());
        let write = Rc::new(RecordRead::new());
        let (mut dom, _) = mount_writer({
            let reads = reads.clone();
            let write = write.clone();
            move || {
                init_i18n();
                use_context_provider(|| reads.clone());
                use_context_provider(|| CredentialReply(write.clone()));
                let host = use_context::<PendingCommandHost>();
                rsx! {
                    PendingCommandNotices { host }
                    {page()}
                }
            }
        });
        let projection = |credential| AdminCredentialPage {
            credentials: vec![credential],
            page: 1,
            page_size: 10,
            total: 1,
            restart_required: false,
        };
        reads.respond(Ok(projection(credential())));
        settle_writer(&mut dom);
        click_button(&mut dom, "停用");
        let mut accepted = credential();
        accepted.status = "disabled".to_owned();
        accepted.state_version = 2;
        write.respond(Ok(AdminCredentialMutationResponse {
            credential: accepted.clone(),
            restart_required: true,
        }));
        settle_writer(&mut dom);
        let receipt = "操作已完成。请重启 Worker 以应用凭据变更。";
        assert!(has_live_text(&dom, receipt));
        reads.respond_unavailable();
        settle_writer(&mut dom);
        assert!(button(&dom, "停用").is_none());
        assert!(has_live_text(&dom, "所需服务暂时不可用。请稍后再试。"));
        assert!(has_live_text(&dom, receipt));
        assert!(button(&dom, "关闭回执").is_some());
        click_button(&mut dom, "重试");
        assert_eq!(reads.started(), 3);
        assert_eq!(write.started(), 1);
        reads.respond(Ok(projection(accepted)));
        settle_writer(&mut dom);
        assert!(button(&dom, "启用").is_some());
        assert!(has_live_text(&dom, receipt));
    }

    #[test]
    fn credential_action_keeps_its_entity_gate_while_an_import_is_unknown() {
        let reads = Rc::new(RecordRead::<AdminCredentialPage>::new());
        let writes = Rc::new(RecordRead::new());
        let (mut view, mounted) = mount_writer({
            let reads = reads.clone();
            let writes = writes.clone();
            move || {
                init_i18n();
                use_context_provider(|| reads.clone());
                use_context_provider(|| CredentialReply(writes.clone()));
                use_hook(|| {
                    AdminApiFailure::stub_write_response(
                        "/api/v1/admin/credentials",
                        Method::POST,
                        503,
                        r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
                    );
                });
                let host = use_context::<PendingCommandHost>();
                rsx! {
                    PendingCommandNotices { host }
                    {page()}
                }
            }
        });
        let projection = |credential| AdminCredentialPage {
            credentials: vec![credential],
            page: 1,
            page_size: 10,
            total: 1,
            restart_required: true,
        };
        reads.respond(Ok(projection(credential())));
        settle_writer(&mut view);

        let form = submit_forms(&mounted).next().unwrap();
        let label = listener(&mounted, "input");
        let cookie =
            text_attributes(&mounted, "id").find(|(_, id)| *id == "import-secret").unwrap().0;
        view.dispatch("input", label, "新的凭据");
        view.dispatch("input", cookie, "fake-test-cookie=second-placeholder");
        view.dispatch("submit", form, "");
        assert!(button(&view, "原样重试").is_some());

        click_button(&mut view, "停用");
        assert_eq!(writes.started(), 1);
        let mut disabled = credential();
        disabled.status = "disabled".to_owned();
        disabled.state_version = 2;
        writes.respond(Ok(AdminCredentialMutationResponse {
            credential: disabled.clone(),
            restart_required: true,
        }));
        settle_writer(&mut view);
        assert_eq!(reads.started(), 2);
        reads.respond(Ok(projection(disabled)));
        settle_writer(&mut view);
        assert!(button(&view, "启用").is_some_and(|(_, disabled)| !disabled));
        assert!(button(&view, "原样重试").is_some());
    }

    #[test]
    fn credential_rename_can_complete_while_import_is_unknown() {
        let reads = Rc::new(RecordRead::<AdminCredentialPage>::new());
        let writes = Rc::new(RecordRead::new());
        let (mut view, mounted) = mount_writer({
            let reads = reads.clone();
            let writes = writes.clone();
            move || {
                init_i18n();
                use_context_provider(|| reads.clone());
                use_context_provider(|| CredentialReply(writes.clone()));
                use_hook(|| {
                    AdminApiFailure::stub_write_response(
                        "/api/v1/admin/credentials",
                        Method::POST,
                        503,
                        r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
                    );
                });
                let host = use_context::<PendingCommandHost>();
                rsx! {
                    PendingCommandNotices { host }
                    {page()}
                }
            }
        });
        reads.respond(Ok(AdminCredentialPage {
            credentials: vec![credential()],
            page: 1,
            page_size: 10,
            total: 1,
            restart_required: false,
        }));
        settle_writer(&mut view);
        let form = submit_forms(&mounted).next().unwrap();
        let cookie =
            text_attributes(&mounted, "id").find(|(_, id)| *id == "import-secret").unwrap().0;
        view.dispatch("input", listener(&mounted, "input"), "新的凭据");
        view.dispatch("input", cookie, "fake-test-cookie=second-placeholder");
        let submitted = view.dispatch("submit", form, "");
        let import_id = text_attributes(&submitted, "data-command-id")
            .next()
            .unwrap()
            .1
            .parse::<uuid::Uuid>()
            .unwrap();

        let dialog = click_button(&mut view, "改名");
        view.dispatch("input", listener(&dialog, "input"), "新标签");
        view.dispatch("submit", submit_forms(&dialog).next().unwrap(), "");
        assert_eq!(writes.started(), 1);
        writes.respond(Ok(AdminCredentialMutationResponse {
            credential: AdminCredential {
                label: "新标签".to_owned(),
                state_version: 2,
                ..credential()
            },
            restart_required: true,
        }));
        settle_writer(&mut view);
        assert_eq!(reads.started(), 2);
        assert!(
            command_button(&view, import_id, "原样重试").is_some_and(|(_, disabled)| !disabled)
        );
    }

    #[test]
    fn credential_actions_wait_for_the_new_version_while_the_old_row_stays_visible() {
        let read = Rc::new(RecordRead::new());
        let written = Rc::new(RecordRead::new());
        let (mut page, _) = mount_writer({
            let read = read.clone();
            move || {
                init_i18n();
                use_context_provider(|| CredentialReply(written.clone()));
                let mut reload = use_signal(|| 0);
                let owner = use_pending_commands(reload, |_| true);
                let pagination =
                    use_admin_pagination("credential-version-test", &["page_size"], None, 100);
                let read = read.clone();
                let rows = use_versioned_admin_page(
                    pagination,
                    reload,
                    || (),
                    move |(), _| {
                        let read = read.clone();
                        async move {
                            if *reload.peek() == 0 { Ok(credential()) } else { read.fetch().await }
                        }
                    },
                );
                rsx! {
                    button { onclick: move |_| reload.set(1), {"使凭据投影失效".to_owned()} }
                    if let Some(Ok((loaded_generation, row))) = rows() {
                        CredentialRow { credential: row, reload_generation: reload, loaded_generation, on_delete: |_| () }
                    }
                    if owner.any_busy() { p { {"已经发送凭据写入".to_owned()} } }
                }
            }
        });
        assert!(!button(&page, "停用").unwrap().1);
        click_button(&mut page, "使凭据投影失效");
        let (stale, disabled) = button(&page, "停用").unwrap();
        assert!(disabled);
        page.dispatch("click", stale, "");
        assert!(!has_live_text(&page, "已经发送凭据写入"));
        let mut updated = credential();
        updated.state_version = 2;
        updated.status = "disabled".to_owned();
        read.respond(Ok(updated));
        settle_writer(&mut page);
        click_button(&mut page, "启用");
        assert!(has_live_text(&page, "已经发送凭据写入"));
    }

    #[component]
    fn CredentialRowFixture() -> Element {
        let owner = use_context::<PendingCommands>();
        let feedback = use_signal(|| None::<CredentialFeedback>);
        rsx! {
            button { onclick: move |_| {
                assert!(submit_credential(owner, credential(), "set_status", None, Some("disabled".to_owned()), feedback));
            }, {"停用测试凭据".to_owned()} }
            if let Some(feedback) = feedback() { p { {feedback.message()} } }
        }
    }

    #[test]
    fn a_removed_credential_row_still_delivers_the_restart_warning() {
        for unload_row in [false, true] {
            let reply = Rc::new(RecordRead::new());
            let (mut page, _) = mount_writer({
                let reply = reply.clone();
                move || {
                    use_context_provider(|| CredentialReply(reply.clone()));
                    let host = use_context::<PendingCommandHost>();
                    let reload = use_signal(|| 0);
                    use_pending_commands(reload, |entity| entity.starts_with("credential:"));
                    let mut row_visible = use_signal(|| true);
                    rsx! {
                        PendingCommandNotices { host }
                        button { onclick: move |_| row_visible.set(false), {"凭据翻页".to_owned()} }
                        if row_visible() { CredentialRowFixture {} }
                    }
                }
            });
            click_button(&mut page, "停用测试凭据");
            if unload_row {
                click_button(&mut page, "凭据翻页");
                assert!(button(&page, "停用测试凭据").is_none());
            }
            let mut updated = credential();
            updated.status = "disabled".to_owned();
            updated.state_version = 2;
            reply.respond(Ok(AdminCredentialMutationResponse {
                credential: updated,
                restart_required: true,
            }));
            settle_writer(&mut page);
            assert!(has_live_text(&page, "操作已完成。请重启 Worker 以应用凭据变更。"));
            assert!(button(&page, "关闭回执").is_some());
        }
    }

    fn rename_dialog() -> Element {
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_ADMIN_LOCALE,
                ADMIN_LOCALE_CATALOG.default_ftl,
                &DEFAULT_ADMIN_LOCALE,
                None,
            )
        })
        .unwrap();
        let submitted = use_context::<Rc<RefCell<Vec<String>>>>();
        let mut open = use_signal(|| true);
        rsx! {
            if open() {
                CredentialRenameDialog {
                    initial_label: "旧标签".to_owned(),
                    input_id: "rename-label".to_owned(),
                    busy: false,
                    on_close: move |_| open.set(false),
                    on_save: move |label| {
                        submitted.borrow_mut().push(label);
                        open.set(false);
                    },
                }
            }
        }
    }

    #[test]
    fn rename_rejects_invalid_labels_before_submitting_and_accepts_corrections() {
        let submitted = Rc::new(RefCell::new(Vec::<String>::new()));
        let mut dom = VirtualDom::new(rename_dialog).with_root_context(submitted.clone());
        let initial = dom.rebuild_to_vec();
        let submit = listener(&initial.edits, "submit");
        let input = listener(&initial.edits, "input");

        dispatch(&dom, input, "input", " ");
        dispatch(&dom, submit, "submit", "");
        assert!(submitted.borrow().is_empty());
        let rejected = dom.render_immediate_to_vec().edits;
        assert!(rejected.iter().any(|edit| matches!(edit,
            Mutation::SetAttribute { id, name: "aria-invalid", value: AttributeValue::Text(value), .. }
                if *id == input && value == "true"
        )));
        assert!(rejected.iter().any(|edit| matches!(edit,
            Mutation::SetAttribute { id, name: "aria-describedby", value: AttributeValue::Text(value), .. }
                if *id == input && value == "rename-label-error"
        )));
        assert!(rejected.iter().any(|edit| matches!(edit,
            Mutation::CreateTextNode { value, .. } if value == "请填写此字段。"
        )));

        dispatch(&dom, input, "input", "仍含空白 ");
        let still_invalid = dom.render_immediate_to_vec().edits;
        assert!(!still_invalid.iter().any(|edit| matches!(edit,
            Mutation::SetAttribute { id, name: "aria-invalid", value: AttributeValue::Text(value), .. }
                if *id == input && value == "false"
        )));
        dispatch(&dom, submit, "submit", "");
        assert!(submitted.borrow().is_empty());

        dispatch(&dom, input, "input", "新标签");
        let corrected = dom.render_immediate_to_vec().edits;
        assert!(corrected.iter().any(|edit| matches!(edit,
            Mutation::SetAttribute { id, name: "aria-invalid", value: AttributeValue::Text(value), .. }
                if *id == input && value == "false"
        )));
        dispatch(&dom, submit, "submit", "");
        assert_eq!(submitted.borrow().as_slice(), ["新标签"]);
    }
}
