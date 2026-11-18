//! Admin 投递详情页：冻结尝试、可引用 Sent 证据、领取状态、处置与来源义务。
//!
//! 只读聚合与人工 Sent 证明入口；所有字段都来自 `auction-contracts` 的安全投影，
//! 页面不展示投递 payload、正文、Cookie 或登录 key，也不在浏览器重建状态机。

use auction_contracts::AdminDeliveryActionBlocker;
use auction_web_shared::{AsyncState, AsyncStateKind, t};
use dioxus::prelude::*;

use super::super::router::Route;
use super::delivery_command_entity;
use super::labels::action_blocker_label;
use super::sent_proof::{SentProofDialog, SentProofSubmission, selectable_evidence};
use crate::api::{
    DeliverySentProofResult, SentProofCommand, acknowledge_delivery, confirm_delivery_sent_proof,
    fetch_delivery_detail, valid_uuid,
};
use crate::surface::pending::{PendingCommandPanel, set_if_alive, use_pending_commands};
use crate::surface::{AdminActionFeedback, admin_api_error_message};

mod sections;

use sections::{
    AttemptSection, CustodySection, DeliverySummary, DispositionSection, EvidenceSection,
    ObligationSection,
};

/// 路由复用的是同一组件实例，因此按投递身份重挂载资源与一次性状态。
pub(crate) fn detail_page(delivery_id: String) -> Element {
    rsx! {
        for delivery_id in [delivery_id] {
            DeliveryDetailPage { key: "{delivery_id}", delivery_id }
        }
    }
}

#[component]
fn DeliveryDetailPage(delivery_id: String) -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let commands = use_pending_commands(refresh, {
        let target = delivery_command_entity(&delivery_id);
        move |entity| entity == target
    });
    let entity = delivery_command_entity(&delivery_id);
    let occupied = commands.contains(&entity);
    let blocked_note = occupied.then_some("admin-delivery-command-occupied");
    let busy = commands.any_busy();
    let mut notice = use_signal(|| None::<AdminActionFeedback>);
    let mut sent_open = use_signal(|| false);
    let mut sent_submitted = use_signal(|| false);
    let acknowledge_reason = use_signal(String::new);
    let acknowledged = use_signal(|| false);

    let detail = use_resource(use_reactive((&delivery_id,), move |(delivery_id,)| {
        let revision = refresh();
        async move {
            #[cfg(test)]
            if let Some(tests::DetailFixture(projection)) =
                try_consume_context::<tests::DetailFixture>()
            {
                return (delivery_id, revision, Ok(projection));
            }
            let result = fetch_delivery_detail(delivery_id.clone()).await;
            (delivery_id, revision, result)
        }
    }));
    let loading = detail.state().cloned() != UseResourceState::Ready;
    let revision = refresh();
    // 刷新信号变化即使旧详情失效，避免资源尚未重启时重新打开旧证据。
    let loaded = if loading {
        None
    } else {
        detail
            .read()
            .as_ref()
            .filter(|(loaded_id, loaded_revision, _)| {
                loaded_id == &delivery_id && *loaded_revision == revision
            })
            .map(|(_, _, result)| result.clone())
    };

    let confirm_delivery_id = delivery_id.clone();
    let confirm_sent_proof = move |submission: SentProofSubmission| {
        let entity = confirm_delivery_id.clone();
        if commands.any_busy() || commands.contains(&delivery_command_entity(&entity)) {
            return;
        }
        notice.set(None);
        sent_submitted.set(false);
        let submitted = commands.submit(
            delivery_command_entity(&entity),
            t!("delivery-sent-proof-confirm"),
            move |command_id| {
                let delivery_id = entity.clone();
                let submission = submission.clone();
                async move {
                    #[cfg(test)]
                    if let Some(requests) = try_consume_context::<tests::SentConfirmRequests>() {
                        return requests.reject();
                    }
                    confirm_delivery_sent_proof(SentProofCommand {
                        command_id,
                        delivery_id,
                        expected_fencing_version: submission.expected_fencing_version,
                        reason: submission.reason,
                        evidence_reference: submission.evidence_reference,
                        evidence: submission.evidence,
                        accept_collected_assets: submission.accept_collected_assets,
                        accept_attempt_history: submission.accept_attempt_history,
                    })
                    .await
                    .map_err(|failure| failure.code)
                }
            },
            move |_: DeliverySentProofResult| {
                set_if_alive(notice, Some(AdminActionFeedback::Success));
                set_if_alive(sent_open, false);
                set_if_alive(sent_submitted, true);
            },
            move |code| {
                if matches!(code.as_str(), "stale_delivery_state" | "stale_state_version") {
                    let mut refresh = refresh;
                    refresh.set(refresh().saturating_add(1));
                }
                set_if_alive(notice, Some(AdminActionFeedback::Error(code)));
            },
        );
        if submitted {
            sent_open.set(false);
        }
    };

    let acknowledge_delivery_id = delivery_id.clone();
    let acknowledge_fencing_version = loaded
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .filter(|projection| projection.actions.acknowledge.blocker().is_none())
        .map(|projection| projection.detail.delivery.fencing_version);
    let acknowledge = move |_| {
        let Some(acknowledge_fencing_version) = acknowledge_fencing_version else {
            return;
        };
        let entity = acknowledge_delivery_id.clone();
        let reason = acknowledge_reason().trim().to_owned();
        if reason.is_empty()
            || commands.any_busy()
            || commands.contains(&delivery_command_entity(&entity))
        {
            return;
        }
        notice.set(None);
        commands.submit(
            delivery_command_entity(&entity),
            t!("delivery-detail-acknowledge-submit"),
            move |command_id| {
                let delivery_id = entity.clone();
                let reason = reason.clone();
                async move {
                    acknowledge_delivery(
                        command_id,
                        delivery_id,
                        acknowledge_fencing_version,
                        reason,
                    )
                    .await
                    .map(|_| ())
                    .map_err(|failure| failure.code)
                }
            },
            move |_: ()| {
                set_if_alive(acknowledged, true);
                set_if_alive(acknowledge_reason, String::new());
                let mut refresh = refresh;
                refresh.set(refresh().saturating_add(1));
            },
            move |code| {
                if matches!(code.as_str(), "stale_delivery_state" | "stale_state_version") {
                    let mut refresh = refresh;
                    refresh.set(refresh().saturating_add(1));
                }
                set_if_alive(notice, Some(AdminActionFeedback::Error(code)));
            },
        );
    };
    let acknowledge_entry = loaded.clone().and_then(std::result::Result::ok).map_or_else(|| rsx! {}, |detail| {
        if let Some(blocker) = detail.actions.acknowledge.blocker() {
            if blocker != AdminDeliveryActionBlocker::StatusMismatch {
                return rsx! {
                    p { class: "admin-panel__note", {action_blocker_label(blocker)} }
                };
            }
            return rsx! {};
        }
        let mut reason = acknowledge_reason;
        let mut submit = acknowledge.clone();
        rsx! {
            section { class: "admin-panel admin-delivery-acknowledge",
                aria_labelledby: "delivery-acknowledge-title",
                h2 { id: "delivery-acknowledge-title", {t!("delivery-detail-acknowledge-title")} }
                p { class: "admin-panel__note", {t!("delivery-detail-acknowledge-intro")} }
                label { class: "admin-field",
                    span { {t!("delivery-detail-acknowledge-reason")} }
                    textarea {
                        rows: "3",
                        maxlength: "2048",
                        disabled: occupied || busy,
                        aria_describedby: blocked_note,
                        value: "{reason}",
                        oninput: move |event| reason.set(event.value()),
                    }
                }
                button {
                    r#type: "button",
                    class: "admin-btn-primary",
                    disabled: occupied || busy || reason().trim().is_empty(),
                    aria_describedby: blocked_note,
                    onclick: move |_| submit(()),
                    {t!("delivery-detail-acknowledge-submit")}
                }
            }
        }
    });

    // 证明入口只有在存在可引用证据且投递处于可准入状态时出现；服务端仍会重新判定。
    let sent_entry = loaded.clone().and_then(std::result::Result::ok).map(|projection| {
        let detail = projection.detail;
        let candidates = !selectable_evidence(&detail).is_empty();
        let can_prove =
            matches!(detail.delivery.status.as_str(), "manual_reconcile" | "failed") && candidates;
        rsx! {
            section { class: "admin-panel admin-delivery-sent-proof-entry",
                aria_labelledby: "delivery-sent-proof-entry-title",
                h2 { id: "delivery-sent-proof-entry-title", {t!("delivery-detail-sent-proof")} }
                p { class: "admin-panel__note", {t!("delivery-detail-sent-proof-intro")} }
                if can_prove {
                    button {
                        r#type: "button",
                        class: "admin-btn-primary",
                        disabled: occupied || busy,
                        aria_describedby: blocked_note,
                        onclick: move |_| {
                            if commands.contains(&entity) || commands.any_busy() {
                                return;
                            }
                            notice.set(None);
                            sent_open.set(true);
                        },
                        {t!("delivery-detail-sent-proof-open")}
                    }
                } else if !candidates {
                    p { class: "admin-panel__note", {t!("delivery-detail-sent-proof-no-evidence")} }
                } else {
                    p { class: "admin-panel__note", {t!("delivery-detail-sent-proof-not-applicable")} }
                }
            }
            if sent_submitted() {
                p { class: "action-feedback action-feedback--success", role: "status",
                    {t!("delivery-sent-proof-submitted")}
                }
            }
            if let Some(feedback) = notice() {
                p {
                    class: if feedback.is_success() {
                        "action-feedback action-feedback--success"
                    } else {
                        "action-feedback action-feedback--error"
                    },
                    role: if feedback.is_success() { "status" } else { "alert" },
                    {feedback.message(t!("delivery-sent-proof-submitted"))}
                }
            }
            if sent_open() {
                SentProofDialog {
                    detail: detail,
                    busy,
                    blocked: occupied,
                    on_submit: confirm_sent_proof,
                    on_cancel: move |_| sent_open.set(false),
                    on_stale: move |_| {
                        // 预览已核对请求序号。关闭旧证据的对话框后，重开的入口只使用新详情。
                        sent_open.set(false);
                        notice.set(Some(AdminActionFeedback::Error("stale_delivery_state".to_owned())));
                        let next = refresh.peek().saturating_add(1);
                        refresh.set(next);
                    },
                }
            }
        }
    });

    rsx! {
        section { class: "admin-delivery-detail", aria_labelledby: "delivery-detail-title",
            div { class: "admin-delivery-detail__head",
                h1 { id: "delivery-detail-title", {t!("delivery-detail-title")} }
                Link { to: Route::Deliveries {}, class: "admin-delivery-detail__back",
                    {t!("delivery-detail-back")}
                }
            }
            PendingCommandPanel { owner: commands }
            if let Some(blocked_note) = blocked_note {
                p { id: blocked_note, class: "admin-panel__note", {t!("delivery-command-occupied")} }
            }
            match loaded {
                None if !valid_uuid(&delivery_id) => rsx! {
                    AsyncState {
                        kind: AsyncStateKind::Error,
                        title: admin_api_error_message("invalid_delivery_id"),
                        detail: "",
                    }
                },
                None => rsx! {
                    AsyncState {
                        kind: AsyncStateKind::Loading,
                        title: t!("delivery-detail-loading"),
                        detail: "",
                    }
                },
                Some(Err(error)) => rsx! {
                    AsyncState {
                        kind: AsyncStateKind::Error,
                        title: admin_api_error_message(&error),
                        detail: "",
                        retry_label: t!("admin-retry"),
                        on_retry: move |_| refresh.set(refresh().saturating_add(1)),
                    }
                },
                Some(Ok(projection)) => rsx! {
                    DeliverySummary { detail: projection.detail.clone() }
                    AttemptSection { attempts: projection.detail.attempts.clone() }
                    EvidenceSection { evidence: projection.detail.evidence.clone() }
                    CustodySection { custody: projection.detail.custody.clone() }
                    DispositionSection { dispositions: projection.detail.dispositions.clone() }
                    ObligationSection {
                        obligation: projection.detail.obligation.clone(),
                        recovery: projection.detail.recovery,
                    }
                },
            }
            if acknowledged() {
                p { class: "action-feedback action-feedback--success", role: "status",
                    {t!("delivery-detail-acknowledge-success")}
                }
            }
            {acknowledge_entry}
            {sent_entry}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::future::ready;
    use std::rc::Rc;

    use auction_contracts::AdminDeliveryDetailWithActionsResponse;
    use dioxus::core::Mutation;
    use dioxus::prelude::*;
    use serde_json::{Value, json};

    use super::DeliveryDetailPage;
    use crate::api::{AdminApiFailure, DeliverySentProofResult};
    use crate::surface::auctions::draft_page_tests::{
        button, click_button, has_live_text, init_i18n, mount_writer, submit_button_disabled,
        writer_form,
    };
    use crate::surface::deliveries::delivery_command_entity;
    use crate::surface::deliveries::sent_proof::use_preview_stub;
    use crate::surface::pending::use_pending_commands;

    #[derive(Clone)]
    pub(super) struct DetailFixture(pub(super) AdminDeliveryDetailWithActionsResponse);

    #[derive(Clone, Default)]
    pub(super) struct SentConfirmRequests(Rc<Cell<usize>>);

    impl SentConfirmRequests {
        pub(super) fn reject(&self) -> Result<DeliverySentProofResult, String> {
            self.0.set(self.0.get() + 1);
            Err(AdminApiFailure::from_http_body(
                429,
                r#"{"code":"rate_limited","message":"请求过多","data":null,"request_id":null}"#,
            )
            .code)
        }
    }

    #[derive(Clone, Debug, PartialEq, Routable)]
    enum TestRoute {
        #[route("/")]
        Fixture {},
    }

    #[component]
    fn Fixture() -> Element {
        let projection = use_context::<DetailFixture>();
        rsx! { DeliveryDetailPage { delivery_id: projection.0.detail.delivery.id } }
    }

    #[test]
    fn another_sources_pending_delivery_blocks_detail_entries_and_the_open_dialog() {
        let [mut projection, _, _]: [Value; 3] = serde_json::from_str(include_str!(
            "../../../../../auction-contracts/tests/fixtures/admin/delivery-sent-proof-detail.json"
        ))
        .unwrap();
        projection["data"]["actions"]["acknowledge"] = json!({"state":"eligible"});
        let fixture = DetailFixture(serde_json::from_value(projection["data"].take()).unwrap());
        let confirms = SentConfirmRequests::default();
        let (mut page, edits) = mount_writer({
            let confirms = confirms.clone();
            move || {
                init_i18n();
                use_preview_stub();
                use_context_provider(|| fixture.clone());
                use_context_provider(|| confirms.clone());
                let refresh = use_signal(|| 0);
                let other_source = use_pending_commands(refresh, |_| false);
                let entity = delivery_command_entity(&fixture.0.detail.delivery.id);
                rsx! {
                    button { onclick: move |_| {
                        let failure = AdminApiFailure::from_http_body(
                            503, r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
                        );
                        assert!(other_source.submit(entity.clone(), "列表处置".to_owned(), move |_| ready(Err::<(), _>(failure.code.clone())), |()| {}, |_| {}));
                    }, {"列表提交同一投递".to_owned()} }
                    Router::<TestRoute> {}
                }
            }
        });
        let reason = edits
            .iter()
            .find_map(|edit| match edit {
                Mutation::NewEventListener { name, id } if name == "input" => Some(*id),
                _ => None,
            })
            .unwrap();
        page.dispatch("input", reason, "已核对");
        assert!(!button(&page, "确认已处置").unwrap().1);
        let opened = click_button(&mut page, "打开人工 Sent 证明");
        let form = writer_form(&opened);
        let mut inputs = opened.iter().filter_map(|edit| match edit {
            Mutation::NewEventListener { name, id } if name == "input" => Some(*id),
            _ => None,
        });
        page.dispatch("input", inputs.next().unwrap(), "Sample");
        page.dispatch("input", inputs.next().unwrap(), "12345");
        page.dispatch("input", inputs.next().unwrap(), "2026-11-02T01:14");
        page.dispatch("input", inputs.next_back().unwrap(), "已核对 Sent 实见");
        click_button(&mut page, "预览准入");
        assert!(has_live_text(&page, "准入通过。确认时仍会重新核验快照。"));
        assert_eq!(submit_button_disabled(&page, "确认证明"), Some(false));
        click_button(&mut page, "列表提交同一投递");
        assert!(has_live_text(&page, "这条投递还有未了结的操作，请先在未决区核对。"));
        assert!(has_live_text(&page, "准入通过。确认时仍会重新核验快照。"));
        assert!(button(&page, "确认已处置").unwrap().1);
        assert_eq!(submit_button_disabled(&page, "确认证明"), Some(true));
        page.dispatch("submit", form, "");
        assert_eq!(confirms.0.get(), 0);
        assert!(button(&page, "打开人工 Sent 证明").unwrap().1);
        click_button(&mut page, "取消");
        let (sent, _) = button(&page, "打开人工 Sent 证明").unwrap();
        page.dispatch("click", sent, "");
        assert!(submit_button_disabled(&page, "确认证明").is_none());
    }
}
