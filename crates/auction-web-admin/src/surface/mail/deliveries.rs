//! 收件关联投递的安全展示与手动重排；原邮件处理结果和外部完成证明仍由后端拥有。

use std::collections::BTreeMap;

use auction_contracts::{AdminDeliveryActionBlocker, AdminDeliveryActionsResponse};
use auction_web_shared::{TableColumns, TruncatedText, t};
use dioxus::prelude::*;

use super::super::deliveries::delivery_command_entity;
use super::super::deliveries::dialog::{
    DeliveryConfirmationFacts, DeliveryEvidenceStatus, ReconcileEvidenceCandidates,
};
use super::super::deliveries::labels::{action_blocker_label, delivery_status_label};
use super::super::deliveries::modal::{DeliveryModal, close_delivery_modal};
use crate::api::{
    AdminMailIntakeDelivery, Delivery, DeliveryAction, execute_delivery, fetch_delivery_detail,
};
use crate::surface::AdminActionFeedback;
use crate::surface::pagination::{AdminPager as Pager, use_admin_pagination};
use crate::surface::pending::{CommandDelivery, CommandReceipt, PendingCommands};

const DIALOG_ID: &str = "mail-delivery-action-dialog";

/// 投递 ID 对应的重排反馈及来源邮件 ID。
pub(super) type MailDeliveryNotices = BTreeMap<String, (String, AdminActionFeedback)>;

/// 当前收件的关联投递、重排入口与确认框。
#[component]
pub(super) fn MailDeliveriesPanel(
    intake_id: String,
    deliveries: Vec<AdminMailIntakeDelivery>,
    mut notices: Signal<MailDeliveryNotices>,
    refresh: Signal<u64>,
) -> Element {
    let mut page = use_signal(|| 1_i64);
    let pagination =
        use_admin_pagination("mail-deliveries", &["mail_delivery_page_size"], None, i64::MAX);
    let page_size = pagination.size();
    let commands = use_context::<PendingCommands>();
    let busy = move || commands.any_busy();
    let mut pending = use_signal(|| None::<AdminMailIntakeDelivery>);
    let confirm_intake_id = intake_id;
    let confirm_action =
        move |(delivery, action, operation): (Delivery, DeliveryAction, String)| {
            if busy() {
                return;
            }
            if commands.contains(&delivery_command_entity(&delivery.id))
                || pending().is_none_or(|row| {
                    row.id != delivery.id || row.fencing_version != delivery.fencing_version
                })
            {
                return;
            }
            let success_intake_id = confirm_intake_id.clone();
            let failure_intake_id = confirm_intake_id.clone();
            let success_delivery_id = delivery.id.clone();
            let failure_delivery_id = delivery.id.clone();
            notices.write().remove(&delivery.id);
            // 未决命令持有提交时的投递身份和版本，重试不读取当前详情。
            let submitted = commands.submit_with_receipt(
                delivery_command_entity(&delivery.id),
                retry_label(&operation),
                move |command_id| {
                    let delivery_id = delivery.id.clone();
                    async move {
                        #[cfg(test)]
                        if let Some(replies) = try_consume_context::<tests::LinkedDeliveryReplies>()
                        {
                            return replies.fetch().await;
                        }
                        execute_delivery(
                            command_id,
                            delivery_id,
                            action,
                            delivery.fencing_version,
                            None,
                            None,
                        )
                        .await
                    }
                },
                move |_, source_alive| {
                    // 邮件页仍存活时，成功回执覆盖同一投递的旧错误，即使原面板已卸载。
                    let notice_updated = source_alive
                        && set_delivery_notice(
                            notices,
                            &success_delivery_id,
                            &success_intake_id,
                            AdminActionFeedback::Success,
                        );
                    let delivered = notice_updated && pending.try_peek().is_ok();
                    CommandDelivery::reported(delivered, || {
                        Some(CommandReceipt::Text(t!("admin-mail-delivery-requeued")))
                    })
                },
                move |code| {
                    if matches!(
                        code.as_str(),
                        "stale_delivery_state"
                            | "stale_state_version"
                            | "delivery_recipient_renamed"
                    ) {
                        let mut refresh = refresh;
                        refresh.set(refresh().saturating_add(1));
                    }
                    set_delivery_notice(
                        notices,
                        &failure_delivery_id,
                        &failure_intake_id,
                        AdminActionFeedback::Error(code),
                    );
                },
            );
            if submitted {
                close_delivery_modal(DIALOG_ID);
                pending.set(None);
            }
        };
    let total = i64::try_from(deliveries.len()).unwrap_or(i64::MAX);
    let current_page = page().clamp(1, if total == 0 { 1 } else { 1 + (total - 1) / page_size });
    let offset =
        usize::try_from((current_page - 1).saturating_mul(page_size)).unwrap_or(usize::MAX);
    let visible =
        deliveries.into_iter().skip(offset).take(usize::try_from(page_size).unwrap_or(usize::MAX));

    rsx! {
        section { class: "admin-mail-deliveries", aria_labelledby: "admin-mail-deliveries-title",
            h3 { id: "admin-mail-deliveries-title", {t!("admin-mail-deliveries-title")} }
            p { class: "admin-panel__note", {t!("admin-mail-deliveries-help")} }
            div { class: "admin-shell__table-wrap",
                table { class: "admin-shell__table admin-mail-deliveries__table",
                    aria_label: t!("admin-mail-deliveries-title"),
                    // 操作类型、收件人、任务 ID、状态、尝试、失败原因、操作合计 100%。
                    TableColumns { layout: "admin-mail-deliveries", widths: vec![12, 14, 20, 14, 6, 19, 15] }
                    thead {
                        tr {
                            th { scope: "col", {t!("admin-mail-delivery-operation")} }
                            th { scope: "col", {t!("delivery-to")} }
                            th { scope: "col", {t!("admin-mail-delivery-id")} }
                            th { scope: "col", {t!("delivery-status")} }
                            th { scope: "col", {t!("delivery-attempts")} }
                            th { scope: "col", {t!("delivery-failure-code")} }
                            th { scope: "col", {t!("catalog-actions")} }
                        }
                    }
                    tbody {
                        for delivery in visible {
                            MailDeliveryRow {
                                key: "{delivery.id}",
                                busy: busy() || commands.contains(&delivery_command_entity(&delivery.id)),
                                delivery,
                                on_retry: move |delivery: AdminMailIntakeDelivery| {
                                    if !busy() && !commands.contains(&delivery_command_entity(&delivery.id)) {
                                        notices.write().remove(&delivery.id);
                                        pending.set(Some(delivery));
                                    }
                                },
                            }
                        }
                    }
                }
            }
            if total == 0 {
                p { class: "admin-panel__note", {t!("admin-mail-deliveries-empty")} }
            }
            Pager {
                    pagination,
                    local: true,
                    page: page(),
                    page_size,
                    total: Some(total),
                    label: t!("admin-mail-deliveries-title"),
                    on_page_change: move |next| page.set(next),
            }
            if let Some(delivery) = pending() {
                MailRetryDialog {
                    delivery,
                    busy: busy(),
                    on_submit: confirm_action,
                    on_cancel: move |_| pending.set(None),
                }
            }
        }
    }
}

fn set_delivery_notice(
    mut notices: Signal<MailDeliveryNotices>,
    delivery_id: &str,
    intake_id: &str,
    feedback: AdminActionFeedback,
) -> bool {
    if let Ok(mut notices) = notices.try_write() {
        notices.insert(delivery_id.to_owned(), (intake_id.to_owned(), feedback));
        true
    } else {
        false
    }
}

#[component]
fn MailRetryDialog(
    delivery: AdminMailIntakeDelivery,
    busy: bool,
    on_submit: EventHandler<(Delivery, DeliveryAction, String)>,
    on_cancel: EventHandler<()>,
) -> Element {
    let mut resource = use_resource(use_reactive((&delivery.id,), move |(delivery_id,)| {
        fetch_delivery_detail(delivery_id)
    }));
    let result =
        if resource.state().cloned() == UseResourceState::Ready { resource.cloned() } else { None };
    let evidence_status = match &result {
        None => Some(t!("delivery-reconcile-sightings-loading")),
        Some(Ok(projected))
            if projected.detail.delivery.fencing_version == delivery.fencing_version
                && retry_action(projected.actions) == Ok(DeliveryAction::ReconcileRetry)
                && projected.detail.evidence.is_empty() =>
        {
            Some(t!("delivery-reconcile-sightings-empty"))
        }
        _ => None,
    };
    rsx! {
        DeliveryModal {
            dialog_id: DIALOG_ID,
            title: retry_label(&delivery.operation),
            busy,
            on_cancel,
            DeliveryEvidenceStatus { message: evidence_status }
            div {
                match result {
                    None => rsx! {},
                    Some(Err(_error)) => rsx! {
                        p { class: "action-feedback action-feedback--error", role: "alert", {t!("delivery-reconcile-sightings-error")} }
                        button { r#type: "button", disabled: busy, onclick: move |_| resource.restart(), {t!("admin-retry")} }
                    },
                    Some(Ok(projected)) if projected.detail.delivery.fencing_version != delivery.fencing_version => rsx! {
                        p { class: "action-feedback action-feedback--error", role: "alert", {t!("delivery-action-blocked-stale")} }
                    },
                    Some(Ok(projected)) => {
                        let target = projected.detail.delivery;
                        let operation = delivery.operation;
                        match retry_action(projected.actions) {
                            Ok(action) => rsx! {
                                DeliveryConfirmationFacts { delivery: target.clone() }
                                if action == DeliveryAction::ReconcileRetry {
                                    ReconcileEvidenceCandidates { evidence: projected.detail.evidence }
                                    p { class: "admin-mail-deliveries__warning", {t!("admin-mail-delivery-confirm-unknown")} }
                                }
                                button {
                                    r#type: "button",
                                    class: "admin-btn-primary",
                                    disabled: busy,
                                    onclick: move |_| on_submit.call((target.clone(), action, operation.clone())),
                                    {t!("delivery-confirm")}
                                }
                            },
                            Err(blocker) => rsx! { p { class: "admin-panel__note", {action_blocker_label(blocker)} } },
                        }
                    },
                }
            }
        }
    }
}

#[component]
fn MailDeliveryRow(
    delivery: AdminMailIntakeDelivery,
    busy: bool,
    on_retry: EventHandler<AdminMailIntakeDelivery>,
) -> Element {
    let target = delivery.clone();
    rsx! {
        tr {
            td { TruncatedText { value: operation_label(&delivery.operation) } }
            td { TruncatedText { value: delivery.recipient } }
            td { TruncatedText { value: delivery.id } }
            td { span { class: "admin-delivery-badge", {delivery_status_label(&delivery.status)} } }
            td { "{delivery.attempt_count}" }
            td { TruncatedText { value: delivery.failure_code.unwrap_or_else(|| t!("value-empty")) } }
            td {
                button {
                    r#type: "button",
                    disabled: busy,
                    onclick: move |_| on_retry.call(target.clone()),
                    {t!("delivery-review-actions")}
                }
            }
        }
    }
}

fn retry_action(
    actions: AdminDeliveryActionsResponse,
) -> Result<DeliveryAction, AdminDeliveryActionBlocker> {
    use AdminDeliveryActionBlocker::StatusMismatch;

    match (actions.retry_failed.blocker(), actions.reconcile_retry.blocker()) {
        (None, Some(_)) => Ok(DeliveryAction::Retry),
        (Some(_), None) => Ok(DeliveryAction::ReconcileRetry),
        (Some(StatusMismatch), Some(reason)) | (Some(reason), Some(_)) => Err(reason),
        (None, None) => Err(StatusMismatch),
    }
}

fn operation_label(operation: &str) -> String {
    match operation {
        "send" => t!("admin-mail-delivery-send"),
        "return" => t!("admin-mail-delivery-return"),
        "take" => t!("admin-mail-delivery-take"),
        _ => t!("value-empty"),
    }
}

fn retry_label(operation: &str) -> String {
    match operation {
        "send" => t!("admin-mail-delivery-resend"),
        "return" => t!("admin-mail-delivery-return-again"),
        "take" => t!("admin-mail-delivery-take-again"),
        _ => t!("value-empty"),
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use auction_contracts::{
        AdminDeliveryActionAvailability, AdminDeliveryActionBlocker, AdminDeliveryActionsResponse,
        AdminDeliveryDetailWithActionsResponse,
    };
    use dioxus::prelude::*;
    use gloo_net::http::Method;
    use serde_json::{Value, json};

    use super::{MailDeliveriesPanel, MailDeliveryNotices, retry_action};
    use crate::api::{
        AdminApiFailure, AdminMailIntakeDelivery, AdminWriteEndpoint, AdminWriteFailure, Delivery,
        DeliveryAction, DeliveryDetailGetFixture,
    };
    use crate::surface::AdminActionFeedback;
    use crate::surface::auctions::draft_page_tests::{
        RecordRead, button, click_button, command_button, has_live_text, init_i18n, mount_writer,
        settle_writer,
    };
    use crate::surface::deliveries::labels::action_feedback_message;
    use crate::surface::pending::{
        PendingCommandHost, PendingCommandNotices, PendingCommandPanel, use_pending_commands,
    };
    use crate::surface::realtime::text_attributes;

    pub(super) type LinkedDeliveryReplies = Rc<RecordRead<Delivery, AdminWriteFailure>>;

    #[test]
    fn linked_mail_uses_the_fence_decision_for_retry() {
        let mismatch = AdminDeliveryActionAvailability::Blocked {
            reason: AdminDeliveryActionBlocker::StatusMismatch,
        };
        let mut actions = AdminDeliveryActionsResponse {
            release: mismatch,
            retry_failed: mismatch,
            reconcile_retry: AdminDeliveryActionAvailability::Eligible {},
            reconcile_succeeded: mismatch,
            reconcile_failed: mismatch,
            acknowledge: mismatch,
        };
        assert_eq!(retry_action(actions), Ok(DeliveryAction::ReconcileRetry));
        actions.reconcile_retry = AdminDeliveryActionAvailability::Blocked {
            reason: AdminDeliveryActionBlocker::SettlementReturnActionOwned,
        };
        assert_eq!(
            retry_action(actions),
            Err(AdminDeliveryActionBlocker::SettlementReturnActionOwned)
        );
    }

    #[test]
    fn linked_mail_retry_delivers_success_after_the_original_panel_unmounts() {
        const A: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
        const B: &str = "00000000-0000-4000-8000-000000000031";
        const INTAKE_A: &str = "00000000-0000-4000-8000-000000000022";
        const INTAKE_B: &str = "00000000-0000-4000-8000-000000000021";
        const SUCCESS: &str = "已重新排队，等待后台执行；这不表示邮件已发送、退回或领取成功。";
        const TIMEOUT: &str = "请求超时，服务端结果可能尚未返回。表单内容已保留，请在当前页面再次提交；相同命令不会重复录入。";
        const STALE: &str =
            "投递状态或证据已变化，请核对最新详情后重试。提交 Sent 证明时请重新选择证据。";
        let [mut wire, _, _]: [Value; 3] = serde_json::from_str(include_str!(
            "../../../../auction-contracts/tests/fixtures/admin/delivery-sent-proof-detail.json"
        ))
        .unwrap();
        wire["data"]["actions"]["reconcile_retry"] = json!({"state":"eligible"});
        let projection: AdminDeliveryDetailWithActionsResponse =
            serde_json::from_value(wire["data"].take()).unwrap();
        let success_a = Delivery {
            id: A.to_owned(),
            fencing_version: 5,
            status: "queued".to_owned(),
            ..projection.detail.delivery.clone()
        };
        let replies: LinkedDeliveryReplies = Rc::new(RecordRead::new());
        let (mut dom, _) = mount_writer({
            let replies = replies.clone();
            move || {
                init_i18n();
                use_context_provider(|| replies.clone());
                use_context_provider(|| {
                    let projection = projection.clone();
                    let get: DeliveryDetailGetFixture = Rc::new(move |url| {
                        let mut detail = projection.clone();
                        if url == format!("/api/v1/admin/deliveries/{B}?include=sent_proof") {
                            detail.detail.delivery.id = B.to_owned();
                        } else {
                            assert_eq!(
                                url,
                                format!("/api/v1/admin/deliveries/{A}?include=sent_proof")
                            );
                        }
                        detail
                    });
                    get
                });
                let notices = use_signal(MailDeliveryNotices::new);
                let refresh = use_signal(|| 0);
                use_pending_commands(refresh, |_| false);
                let host = use_context::<PendingCommandHost>();
                let mut active = use_signal(|| A);
                let intake = if active() == A { INTAKE_A } else { INTAKE_B };
                let delivery = AdminMailIntakeDelivery {
                    id: active().to_owned(),
                    operation: "send".to_owned(),
                    recipient: "Sample".to_owned(),
                    status: "manual_reconcile".to_owned(),
                    fencing_version: 4,
                    attempt_count: 1,
                    failure_code: Some("write_unknown".to_owned()),
                    reason_code: None,
                    external_reference: None,
                };
                let notice = notices.read().get(active()).filter(|(owner, _)| owner == intake).map(
                    |(_, feedback)| {
                        let message = action_feedback_message(feedback, SUCCESS.to_owned());
                        format!("{} · {message}", active())
                    },
                );
                rsx! {
                    PendingCommandNotices { host }
                    button { onclick: move |_| active.set(if active() == A { B } else { A }), {"切换关联投递".to_owned()} }
                    for id in [active()] {
                        MailDeliveriesPanel { key: "{id}", intake_id: intake.to_owned(), deliveries: vec![delivery.clone()], notices, refresh }
                    }
                    if let Some(notice) = notice {
                        p { "{notice}" }
                    }
                }
            }
        });
        click_button(&mut dom, "查看处置");
        settle_writer(&mut dom);
        click_button(&mut dom, "确认");
        assert_eq!(replies.started(), 1);
        replies.respond(Err(AdminWriteFailure::outcome_unknown("request_timeout")));
        let edits = settle_writer(&mut dom);
        let command_id =
            text_attributes(&edits, "data-command-id").next().unwrap().1.parse().unwrap();
        assert!(has_live_text(&dom, &format!("{A} · {TIMEOUT}")));
        assert!(button(&dom, "关闭回执").is_none());

        click_button(&mut dom, "切换关联投递");
        click_button(&mut dom, "查看处置");
        settle_writer(&mut dom);
        click_button(&mut dom, "确认");
        replies.respond(Err(AdminWriteFailure::from_test_http(
            AdminWriteEndpoint::General,
            409,
            r#"{"code":"stale_delivery_state","message":"投递状态已变化","data":null,"request_id":null}"#,
        )));
        settle_writer(&mut dom);
        let notice_b = format!("{B} · {STALE}");
        assert!(has_live_text(&dom, &notice_b));
        assert!(button(&dom, "关闭回执").is_none());

        let (retry, disabled) = command_button(&dom, command_id, "原样重试").unwrap();
        assert!(!disabled);
        dom.dispatch("click", retry, "");
        assert_eq!(replies.started(), 3);
        replies.respond(Ok(success_a));
        settle_writer(&mut dom);
        assert!(has_live_text(&dom, &notice_b), "A 重试成功后 B 的错误提示应保留");
        assert!(has_live_text(&dom, SUCCESS));
        assert!(button(&dom, "原样重试").is_none());
        click_button(&mut dom, "关闭回执");
        assert!(button(&dom, "关闭回执").is_none());
        assert!(!has_live_text(&dom, SUCCESS));

        click_button(&mut dom, "切换关联投递");
        assert!(has_live_text(&dom, &format!("{A} · {SUCCESS}")));
        assert!(!has_live_text(&dom, &format!("{A} · {TIMEOUT}")));
    }

    #[test]
    fn linked_mail_failure_tracks_the_original_delivery_after_panel_unmount() {
        const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
        const INTAKE_ID: &str = "00000000-0000-4000-8000-000000000022";
        const PATH: &str =
            "/api/v1/admin/deliveries/0f8fad5b-d9cb-469f-a165-70867728950e/reconcile";
        let [mut wire, _, _]: [Value; 3] = serde_json::from_str(include_str!(
            "../../../../auction-contracts/tests/fixtures/admin/delivery-sent-proof-detail.json"
        ))
        .unwrap();
        wire["data"]["actions"]["reconcile_retry"] = json!({"state":"eligible"});
        let read: AdminDeliveryDetailWithActionsResponse =
            serde_json::from_value(wire["data"].take()).unwrap();
        let delivery = AdminMailIntakeDelivery {
            id: ID.to_owned(),
            operation: "send".to_owned(),
            recipient: "Sample".to_owned(),
            status: "manual_reconcile".to_owned(),
            fencing_version: 4,
            attempt_count: 1,
            failure_code: Some("write_unknown".to_owned()),
            reason_code: None,
            external_reference: None,
        };
        let (mut dom, _) = mount_writer(move || {
            init_i18n();
            use_context_provider(|| {
                let read = read.clone();
                let get: DeliveryDetailGetFixture = Rc::new(move |url| {
                    assert_eq!(url, format!("/api/v1/admin/deliveries/{ID}?include=sent_proof"));
                    read.clone()
                });
                get
            });
            let notices = use_signal(MailDeliveryNotices::new);
            let refresh = use_signal(|| 0);
            let owner = use_pending_commands(refresh, |_| false);
            let mut showing = use_signal(|| true);
            let (status, body) = if showing() {
                (
                    503,
                    r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
                )
            } else {
                (502, "{")
            };
            AdminApiFailure::stub_write_response(PATH, Method::POST, status, body);
            let notice = notices.read().get(ID).and_then(|(intake, feedback)| match feedback {
                AdminActionFeedback::Error(code) => Some(format!("{intake} · {ID} · {code}")),
                AdminActionFeedback::Success => None,
            });
            rsx! {
                PendingCommandPanel { owner }
                button { onclick: move |_| showing.set(!showing()), {"切换关联投递".to_owned()} }
                if showing() {
                    MailDeliveriesPanel { intake_id: INTAKE_ID.to_owned(), deliveries: vec![delivery.clone()], notices, refresh }
                    if let Some(notice) = notice {
                        p { "{notice}" }
                    }
                }
            }
        });
        click_button(&mut dom, "查看处置");
        settle_writer(&mut dom);
        click_button(&mut dom, "确认");
        settle_writer(&mut dom);
        let notice = format!("{INTAKE_ID} · {ID} · storage_unavailable");
        assert!(has_live_text(&dom, &notice));
        click_button(&mut dom, "切换关联投递");
        assert!(!has_live_text(&dom, &notice));
        click_button(&mut dom, "原样重试");
        settle_writer(&mut dom);
        assert!(button(&dom, "原样重试").is_some());
        click_button(&mut dom, "切换关联投递");
        assert!(!has_live_text(&dom, &notice));
        assert!(has_live_text(&dom, &format!("{INTAKE_ID} · {ID} · http_502")));
    }
}
