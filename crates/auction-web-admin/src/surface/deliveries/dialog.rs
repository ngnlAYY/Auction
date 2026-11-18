//! 投递动作确认与人工对账的当前 fence 证据。

use std::rc::Rc;

use auction_contracts::{
    ADMIN_DELIVERY_RECONCILE_FAILURE_CODES, AdminDeliveryActionAvailability,
    AdminDeliveryActionsResponse,
};
use auction_web_shared::t;
use dioxus::html::MountedData;
use dioxus::prelude::*;

use super::labels::{action_blocker_label, delivery_display_id, delivery_status_label};
use super::modal::{DeliveryModal, close_delivery_modal};
use super::model::{DeliveryActionRequest, DeliveryDialogKind, DeliveryReconcileDraft};
use super::sent_proof::evidence_kind_label;
use crate::api::{Delivery, DeliveryAction, DeliveryEvidence, fetch_delivery_detail};
use crate::surface::format_admin_time;

pub(super) const DIALOG_ID: &str = "delivery-action-dialog";

/// 对当前 fence 的证据与准入分别展示读取结果；未取得当前判定时不提交操作。
#[component]
pub(super) fn DeliveryDialog(
    delivery: Delivery,
    kind: DeliveryDialogKind,
    busy: bool,
    draft: DeliveryReconcileDraft,
    on_draft_change: EventHandler<DeliveryReconcileDraft>,
    on_confirm: EventHandler<DeliveryActionRequest>,
    on_cancel: EventHandler<()>,
) -> Element {
    let title = match kind {
        DeliveryDialogKind::Release => t!("delivery-release"),
        DeliveryDialogKind::Retry | DeliveryDialogKind::ReconcileRetry => t!("delivery-retry"),
        DeliveryDialogKind::Reconcile => {
            t!("delivery-reconcile-title", id: delivery_display_id(&delivery))
        }
    };
    let mut resource = use_resource(use_reactive((&delivery.id,), move |(delivery_id,)| {
        fetch_delivery_detail(delivery_id)
    }));
    let result =
        if resource.state().cloned() == UseResourceState::Ready { resource.cloned() } else { None };
    let evidence_status = match &result {
        None => Some(t!("delivery-reconcile-sightings-loading")),
        Some(Ok(projected))
            if kind == DeliveryDialogKind::Reconcile
                && same_fence(&delivery, &projected.detail.delivery)
                && projected.detail.evidence.is_empty() =>
        {
            Some(t!("delivery-reconcile-sightings-empty"))
        }
        _ => None,
    };
    let mut cancel_button = use_cancel_button_focus(kind == DeliveryDialogKind::ReconcileRetry);
    let cancel = move |_| {
        close_delivery_modal(DIALOG_ID);
        on_cancel.call(());
    };
    rsx! {
        DeliveryModal {
            dialog_id: DIALOG_ID,
            title,
            busy,
            on_cancel,
            DeliveryEvidenceStatus { message: evidence_status }
            div {
                match result {
                    None => rsx! {},
                    Some(Err(_error)) => rsx! {
                        p { class: "action-feedback action-feedback--error", role: "alert",
                            {t!("delivery-reconcile-sightings-error")}
                        }
                        button { r#type: "button", disabled: busy, onclick: move |_| resource.restart(),
                            {t!("admin-retry")}
                        }
                    },
                    Some(Ok(projected)) if !same_fence(&delivery, &projected.detail.delivery) => rsx! {
                        p { class: "action-feedback action-feedback--error", role: "alert",
                            {t!("delivery-action-blocked-stale")}
                        }
                    },
                    Some(Ok(projected)) if kind == DeliveryDialogKind::Reconcile => rsx! {
                        ReconcileEvidenceCandidates { evidence: projected.detail.evidence }
                        ReconcileForm {
                            delivery: projected.detail.delivery,
                            actions: projected.actions,
                            busy, draft, on_draft_change, on_confirm,
                        }
                    },
                    Some(Ok(projected)) => {
                        let action = kind.action();
                        let delivery = projected.detail.delivery;
                        rsx! {
                            DeliveryConfirmationFacts { delivery: delivery.clone() }
                            if let Some(blocker) = availability(projected.actions, action).blocker() {
                                p { class: "admin-panel__note", {action_blocker_label(blocker)} }
                            } else {
                                button {
                                    r#type: "button",
                                    class: "admin-btn-primary",
                                    disabled: busy,
                                    onclick: move |_| on_confirm.call(DeliveryActionRequest {
                                        delivery_id: delivery.id.clone(),
                                        action,
                                        expected_fencing_version: delivery.fencing_version,
                                        external_reference: None,
                                        failure_code: None,
                                    }),
                                    {t!("delivery-confirm")}
                                }
                            }
                        }
                    },
                }
            }
            div { class: "admin-shell__dialog-actions",
                button {
                    r#type: "button",
                    disabled: busy,
                    onmounted: move |event| cancel_button.set(Some(event.data())),
                    onclick: cancel,
                    {t!("delivery-cancel")}
                }
            }
        }
    }
}

fn use_cancel_button_focus(second_step: bool) -> Signal<Option<Rc<MountedData>>> {
    let button = use_signal(|| None::<Rc<MountedData>>);
    // 第二步复用已经打开的对话框；第一步的控件卸载后，焦点回到安全动作。
    use_effect(use_reactive((&second_step,), move |(second_step,)| {
        if second_step && let Some(button) = button() {
            spawn(async move {
                button.set_focus(true).await.unwrap_or_default();
            });
        }
    }));
    button
}

/// 先挂载播报区，再更新证据状态，首次打开也能播报正在读取。
#[component]
pub(crate) fn DeliveryEvidenceStatus(message: Option<String>) -> Element {
    let mut announced = use_signal(|| None::<String>);
    use_effect(use_reactive((&message,), move |(message,)| announced.set(message)));
    rsx! {
        div { role: "status", aria_atomic: "true",
            if let Some(message) = announced() {
                p { class: "admin-panel__note", "{message}" }
            }
        }
    }
}

fn same_fence(expected: &Delivery, current: &Delivery) -> bool {
    expected.id == current.id && expected.fencing_version == current.fencing_version
}

fn availability(
    actions: AdminDeliveryActionsResponse,
    action: DeliveryAction,
) -> AdminDeliveryActionAvailability {
    match action {
        DeliveryAction::Release => actions.release,
        DeliveryAction::Retry => actions.retry_failed,
        DeliveryAction::ReconcileRetry => actions.reconcile_retry,
        DeliveryAction::ReconcileSucceeded => actions.reconcile_succeeded,
        DeliveryAction::ReconcileFailed => actions.reconcile_failed,
    }
}

fn selected_reconcile_action(
    selection: &str,
    actions: AdminDeliveryActionsResponse,
) -> Option<DeliveryAction> {
    let action = match selection {
        "retry" => DeliveryAction::ReconcileRetry,
        "succeeded" => DeliveryAction::ReconcileSucceeded,
        "failed" => DeliveryAction::ReconcileFailed,
        _ => return None,
    };
    availability(actions, action).blocker().is_none().then_some(action)
}

#[component]
fn ReconcileForm(
    delivery: Delivery,
    actions: AdminDeliveryActionsResponse,
    busy: bool,
    draft: DeliveryReconcileDraft,
    on_draft_change: EventHandler<DeliveryReconcileDraft>,
    on_confirm: EventHandler<DeliveryActionRequest>,
) -> Element {
    let options = [
        ("retry", t!("delivery-reconcile-action-retry"), actions.reconcile_retry),
        ("succeeded", t!("delivery-reconcile-action-succeeded"), actions.reconcile_succeeded),
        ("failed", t!("delivery-reconcile-action-failed"), actions.reconcile_failed),
    ];
    if options.iter().all(|(_, _, decision)| decision.blocker().is_some()) {
        return rsx! {
            for (value, label, decision) in options {
                if let Some(blocker) = decision.blocker() {
                    p { key: "{value}", class: "admin-panel__note", "{label} · {action_blocker_label(blocker)}" }
                }
            }
        };
    }
    let selection = draft.action.clone();
    let selected = selected_reconcile_action(&selection, actions);
    let action_draft = draft.clone();
    let reference_draft = draft.clone();
    let failure_draft = draft.clone();
    let submit_draft = draft.clone();
    let target_id = delivery.id;
    rsx! {
        form {
            class: "admin-delivery-reconcile",
            onsubmit: move |event| {
                event.prevent_default();
                let Some(action) = selected.filter(|_| !busy) else { return };
                let external = submit_draft.external_reference.trim();
                let failure = &submit_draft.failure_code;
                if (action == DeliveryAction::ReconcileSucceeded && external.is_empty())
                    || (action == DeliveryAction::ReconcileFailed
                        && !ADMIN_DELIVERY_RECONCILE_FAILURE_CODES.contains(&failure.as_str()))
                {
                    return;
                }
                on_confirm.call(DeliveryActionRequest {
                    delivery_id: target_id.clone(),
                    action,
                    expected_fencing_version: delivery.fencing_version,
                    external_reference: (action == DeliveryAction::ReconcileSucceeded).then(|| external.to_owned()),
                    failure_code: (action == DeliveryAction::ReconcileFailed).then(|| failure.clone()),
                });
            },
            label { r#for: "delivery-reconcile-action", {t!("delivery-reconcile-action")} }
            select {
                id: "delivery-reconcile-action",
                required: true,
                value: selection,
                onchange: move |event| on_draft_change.call(DeliveryReconcileDraft {
                    action: event.value(),
                    ..action_draft.clone()
                }),
                option { value: "", disabled: true, selected: selected.is_none(), {t!("delivery-reconcile-action-placeholder")} }
                for (value, label, decision) in &options {
                    if decision.blocker().is_none() {
                        option { key: "{value}", value: *value, selected: selection == *value, "{label}" }
                    }
                }
            }
            for (value, label, decision) in options {
                if let Some(blocker) = decision.blocker() {
                    p { key: "{value}", class: "admin-panel__note", "{label} · {action_blocker_label(blocker)}" }
                }
            }
            if selected == Some(DeliveryAction::ReconcileSucceeded) {
                label { r#for: "delivery-external-reference", {t!("delivery-external-reference")} }
                input {
                    id: "delivery-external-reference",
                    r#type: "text",
                    required: true,
                    value: draft.external_reference.clone(),
                    oninput: move |event| on_draft_change.call(DeliveryReconcileDraft {
                        external_reference: event.value(),
                        ..reference_draft.clone()
                    }),
                }
            }
            if selected == Some(DeliveryAction::ReconcileFailed) {
                label { r#for: "delivery-failure-code", {t!("delivery-failure-code")} }
                select {
                    id: "delivery-failure-code",
                    required: true,
                    value: draft.failure_code.clone(),
                    onchange: move |event| on_draft_change.call(DeliveryReconcileDraft {
                        failure_code: event.value(),
                        ..failure_draft.clone()
                    }),
                    option {
                        value: "", disabled: true, selected: draft.failure_code.is_empty(),
                        {t!("delivery-failure-code-placeholder")}
                    }
                    for code in ADMIN_DELIVERY_RECONCILE_FAILURE_CODES {
                        option {
                            key: "{code}", value: code, selected: draft.failure_code == code,
                            {failure_code_label(code)}
                        }
                    }
                }
            }
            button {
                r#type: "submit",
                class: "admin-btn-primary",
                disabled: busy,
                if selected == Some(DeliveryAction::ReconcileRetry) {
                    {t!("delivery-retry")}
                } else {
                    {t!("delivery-reconcile")}
                }
            }
        }
    }
}

/// 进入重新排队确认前展示本次操作绑定的真实投递身份和状态。
#[component]
pub(crate) fn DeliveryConfirmationFacts(delivery: Delivery) -> Element {
    rsx! {
        dl { class: "admin-delivery-detail__grid",
            dt { {t!("admin-mail-delivery-id")} }
            dd { code { "{delivery.id}" } }
            dt { {t!("delivery-to")} }
            dd { "{delivery.recipient}" }
            dt { {t!("delivery-status")} }
            dd { {delivery_status_label(&delivery.status)} }
            dt { {t!("delivery-template")} }
            dd { {delivery.template.unwrap_or_else(|| t!("value-empty"))} }
            if let Some(failure) = delivery.failure_code {
                dt { {t!("delivery-failure-code")} }
                dd { "{failure}" }
            }
            if let Some(reference) = delivery.external_reference {
                dt { {t!("delivery-external-reference")} }
                dd { "{reference}" }
            }
        }
        p { class: "admin-panel__note", {t!("delivery-requeue-consequence")} }
    }
}

#[component]
pub(crate) fn ReconcileEvidenceCandidates(evidence: Vec<DeliveryEvidence>) -> Element {
    rsx! {
        section { class: "admin-delivery-reconcile__evidence",
            h3 { {t!("delivery-reconcile-sightings")} }
            p { class: "admin-panel__note", {t!("delivery-reconcile-sightings-help")} }
            if !evidence.is_empty() {
                ul { class: "admin-delivery-reconcile__sightings",
                    for item in evidence {
                        li { key: "{item.reference}",
                            span { class: "admin-delivery-reconcile__kind", {evidence_kind_label(&item.kind)} }
                            code { {item.external_mail_id.clone().unwrap_or_else(|| t!("value-empty"))} }
                            span { {observed_label(&item.observed_at)} }
                            span { {collected_label(item.collected)} }
                        }
                    }
                }
            }
        }
    }
}

fn observed_label(value: &Option<String>) -> String {
    value.as_deref().map_or_else(|| t!("value-empty"), format_admin_time)
}

fn collected_label(value: Option<bool>) -> String {
    match value {
        Some(true) => t!("delivery-detail-yes"),
        Some(false) => t!("delivery-detail-no"),
        None => t!("delivery-detail-custody-unobserved"),
    }
}

/// 闭集失败原因码的界面译文；未知码原样显示，不猜测语义。
fn failure_code_label(code: &str) -> String {
    match code {
        "verified_not_sent" => t!("delivery-failure-code-verified-not-sent"),
        "recipient_invalid" => t!("delivery-failure-code-recipient-invalid"),
        "no_longer_needed" => t!("delivery-failure-code-no-longer-needed"),
        other => other.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::cell::Cell;
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;

    use auction_contracts::{
        AdminDeliveryActionBlocker, AdminDeliveryDetailWithActionsResponse, ApiResponse,
    };
    use dioxus::dioxus_core::Mutation;
    use dioxus::html::{MountedData, MountedResult, RenderedElementBacking};
    use dioxus::prelude::*;

    use super::{
        AdminDeliveryActionAvailability, AdminDeliveryActionsResponse, DeliveryAction,
        DeliveryEvidenceStatus, DeliveryReconcileDraft, selected_reconcile_action,
        use_cancel_button_focus,
    };

    #[derive(Default)]
    struct FocusControl {
        second_step: Cell<Option<Signal<bool>>>,
        button: Cell<Option<Signal<Option<Rc<MountedData>>>>>,
    }

    #[component]
    fn CancelFocusHarness() -> Element {
        let second_step = use_signal(|| false);
        let control = consume_context::<Rc<FocusControl>>();
        control.second_step.set(Some(second_step));
        control.button.set(Some(use_cancel_button_focus(second_step())));
        rsx! { button { "取消" } }
    }

    struct FocusProbe(Rc<Cell<u32>>);

    impl RenderedElementBacking for FocusProbe {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn set_focus(&self, focused: bool) -> Pin<Box<dyn Future<Output = MountedResult<()>>>> {
            if focused {
                self.0.set(self.0.get() + 1);
            }
            Box::pin(std::future::ready(Ok(())))
        }
    }

    #[test]
    fn advancing_confirmation_focuses_the_existing_cancel_button() {
        let control = Rc::new(FocusControl::default());
        let mut dom = VirtualDom::new(CancelFocusHarness).with_root_context(control.clone());
        dom.rebuild_to_vec();
        let focused = Rc::new(Cell::new(0));
        let mounted = Rc::new(MountedData::from(FocusProbe(focused.clone())));
        let mut button = control.button.get().unwrap();
        dom.in_runtime(|| button.set(Some(mounted)));
        dom.process_events();
        assert_eq!(focused.get(), 0);

        let mut second_step = control.second_step.get().unwrap();
        dom.in_runtime(|| second_step.set(true));
        dom.render_immediate_to_vec();
        dom.process_events();
        assert_eq!(focused.get(), 1);
    }

    type EvidenceControl = Rc<Cell<Option<Signal<&'static str>>>>;

    #[component]
    fn EvidenceStatusHarness() -> Element {
        let message = use_signal(|| "正在读取证据");
        consume_context::<EvidenceControl>().set(Some(message));
        rsx! { DeliveryEvidenceStatus { message: Some(message().to_owned()) } }
    }

    #[test]
    fn evidence_status_announces_loading_and_empty_in_the_same_region() {
        let control = EvidenceControl::default();
        let mut dom = VirtualDom::new(EvidenceStatusHarness).with_root_context(control.clone());
        let initial = dom.rebuild_to_vec().edits;
        let region = initial
            .iter()
            .find_map(|edit| match edit {
                Mutation::LoadTemplate { id, .. } => Some(*id),
                _ => None,
            })
            .expect("挂载状态播报区");
        assert!(!contains_status_text(&initial, "正在读取证据"));
        let loading = dom.render_immediate_to_vec().edits;
        assert!(contains_status_text(&loading, "正在读取证据"));

        let mut message = control.get().unwrap();
        dom.in_runtime(|| message.set("没有证据"));
        let mut update = dom.render_immediate_to_vec().edits;
        update.extend(dom.render_immediate_to_vec().edits);
        assert!(contains_status_text(&update, "没有证据"));
        assert!(loading.iter().chain(&update).all(|edit| !matches!(
            edit,
            Mutation::Remove { id } | Mutation::ReplaceWith { id, .. } if *id == region
        )));
    }

    fn contains_status_text(edits: &[Mutation], expected: &str) -> bool {
        edits.iter().any(|edit| {
            matches!(
                edit,
                Mutation::CreateTextNode { value, .. } | Mutation::SetText { value, .. }
                    if value == expected
            )
        })
    }

    #[test]
    fn fresh_reconcile_draft_has_no_default_action() {
        let fixture: ApiResponse<AdminDeliveryDetailWithActionsResponse> = serde_json::from_str(
            include_str!("../../api/delivery/blocked_registration_reply.json"),
        )
        .unwrap();
        let draft = DeliveryReconcileDraft::for_delivery(&fixture.data.detail.delivery);
        assert!(draft.action.is_empty());
    }

    #[test]
    fn reconcile_requires_an_explicit_server_eligible_choice() {
        let blocked = AdminDeliveryActionAvailability::Blocked {
            reason: AdminDeliveryActionBlocker::SentSighted,
        };
        let mut actions = AdminDeliveryActionsResponse {
            release: blocked,
            retry_failed: blocked,
            reconcile_retry: AdminDeliveryActionAvailability::Eligible {},
            reconcile_succeeded: blocked,
            reconcile_failed: blocked,
            acknowledge: blocked,
        };
        assert_eq!(selected_reconcile_action("", actions), None);
        assert_eq!(
            selected_reconcile_action("retry", actions),
            Some(DeliveryAction::ReconcileRetry)
        );
        assert_eq!(selected_reconcile_action("succeeded", actions), None);
        actions.reconcile_retry = blocked;
        assert_eq!(selected_reconcile_action("retry", actions), None);
    }
}
