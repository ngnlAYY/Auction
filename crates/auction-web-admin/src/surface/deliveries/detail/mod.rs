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
use crate::surface::admin_api_error_message;
use crate::surface::pending::{PendingCommandPanel, set_if_alive, use_pending_commands};

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
    let mut action_error = use_signal(|| None::<String>);
    let mut sent_open = use_signal(|| false);
    let mut sent_submitted = use_signal(|| false);
    let acknowledge_reason = use_signal(String::new);
    let acknowledged = use_signal(|| false);

    let detail = use_resource(use_reactive((&delivery_id,), move |(delivery_id,)| {
        let revision = refresh();
        async move {
            let result = fetch_delivery_detail(delivery_id.clone()).await;
            #[cfg(test)]
            let result = tests::apply_detail_get_failure(result);
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
        action_error.set(None);
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
                        return requests.fetch().await;
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
                set_if_alive(sent_open, false);
                set_if_alive(sent_submitted, true);
            },
            move |code| {
                if matches!(code.as_str(), "stale_delivery_state" | "stale_state_version") {
                    let mut refresh = refresh;
                    refresh.set(refresh().saturating_add(1));
                }
                set_if_alive(action_error, Some(code));
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
        action_error.set(None);
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
                set_if_alive(action_error, Some(code));
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
                            action_error.set(None);
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
                        action_error.set(Some("stale_delivery_state".to_owned()));
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
            // 详情与证明入口随读取卸载，已接受的写入仍由投递页面交付。
            div { role: "status",
                if acknowledged() {
                    p { class: "action-feedback action-feedback--success",
                        {t!("delivery-detail-acknowledge-success")}
                    }
                }
                if sent_submitted() {
                    p { class: "action-feedback action-feedback--success",
                        {t!("delivery-sent-proof-submitted")}
                    }
                }
            }
            if let Some(code) = action_error() {
                p { class: "action-feedback action-feedback--error", role: "alert",
                    {admin_api_error_message(&code)}
                }
            }
            {acknowledge_entry}
            {sent_entry}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::future::ready;
    use std::rc::Rc;

    use auction_contracts::AdminDeliveryDetailWithActionsResponse;
    use dioxus::core::{ElementId, Mutation};
    use dioxus::prelude::*;
    use gloo_net::http::Method;
    use serde_json::{Value, json};

    use super::DeliveryDetailPage;
    use crate::api::{AdminApiFailure, DeliveryDetailGetFixture, DeliverySentProofResult};
    use crate::surface::auctions::draft_page_tests::{
        RecordRead, button, click_button, has_live_text, init_i18n, mount_writer, settle_writer,
        submit_button_disabled, writer_form,
    };
    use crate::surface::deliveries::delivery_command_entity;
    use crate::surface::deliveries::sent_proof::use_preview_stub;
    use crate::surface::pending::use_pending_commands;
    use crate::surface::realtime::TestPage;

    #[derive(Clone)]
    struct DetailFixture {
        initial: AdminDeliveryDetailWithActionsResponse,
        refreshed: Option<AdminDeliveryDetailWithActionsResponse>,
        requests: Rc<RefCell<Vec<String>>>,
    }

    impl DetailFixture {
        fn load(&self, url: &str) -> AdminDeliveryDetailWithActionsResponse {
            let mut requests = self.requests.borrow_mut();
            let refreshed = !requests.is_empty();
            requests.push(url.to_owned());
            match &self.refreshed {
                Some(detail) if refreshed => detail.clone(),
                _ => self.initial.clone(),
            }
        }
    }

    fn use_detail_fixture(fixture: &DetailFixture) {
        use_context_provider(|| fixture.clone());
        use_context_provider(|| {
            let fixture = fixture.clone();
            let get: DeliveryDetailGetFixture = Rc::new(move |url| fixture.load(url));
            get
        });
    }

    #[derive(Clone)]
    struct DetailGetFailure {
        request_number: usize,
        code: String,
    }

    pub(super) fn apply_detail_get_failure(
        result: Result<AdminDeliveryDetailWithActionsResponse, String>,
    ) -> Result<AdminDeliveryDetailWithActionsResponse, String> {
        // 详情夹具先登记实际 GET，再让指定的读取失败。
        if let Some(failure) = try_consume_context::<DetailGetFailure>()
            && let Some(fixture) = try_consume_context::<DetailFixture>()
            && fixture.requests.borrow().len() == failure.request_number
        {
            return Err(failure.code);
        }
        result
    }

    pub(super) type SentConfirmRequests = Rc<RecordRead<DeliverySentProofResult>>;

    #[derive(Clone, Debug, PartialEq, Routable)]
    enum TestRoute {
        #[route("/")]
        Fixture {},
    }

    #[component]
    fn Fixture() -> Element {
        let fixture = use_context::<DetailFixture>();
        rsx! { DeliveryDetailPage { delivery_id: fixture.initial.detail.delivery.id } }
    }

    fn fixture() -> DetailFixture {
        let [mut projection, _, _]: [Value; 3] = serde_json::from_str(include_str!(
            "../../../../../auction-contracts/tests/fixtures/admin/delivery-sent-proof-detail.json"
        ))
        .unwrap();
        projection["data"]["actions"]["acknowledge"] = json!({"state":"eligible"});
        DetailFixture {
            initial: serde_json::from_value(projection["data"].take()).unwrap(),
            refreshed: None,
            requests: Rc::default(),
        }
    }

    fn open_reviewed_proof(page: &mut TestPage) -> ElementId {
        let opened = click_button(page, "打开人工 Sent 证明");
        let mut inputs = opened.iter().filter_map(|edit| match edit {
            Mutation::NewEventListener { name, id } if name == "input" => Some(*id),
            _ => None,
        });
        page.dispatch("input", inputs.next().unwrap(), "Sample");
        page.dispatch("input", inputs.next().unwrap(), "12345");
        page.dispatch("input", inputs.next().unwrap(), "2026-11-02T01:14");
        page.dispatch("input", inputs.next_back().unwrap(), "已核对 Sent 实见");
        click_button(page, "预览准入");
        assert!(has_live_text(page, "准入通过。确认时仍会重新核验快照。"));
        assert_eq!(submit_button_disabled(page, "确认证明"), Some(false));
        writer_form(&opened)
    }

    #[test]
    fn another_sources_pending_delivery_blocks_detail_entries_and_the_open_dialog() {
        let fixture = fixture();
        let confirms = Rc::new(RecordRead::<DeliverySentProofResult>::new());
        let (mut page, edits) = mount_writer({
            let confirms = confirms.clone();
            move || {
                init_i18n();
                use_preview_stub();
                use_detail_fixture(&fixture);
                use_context_provider(|| confirms.clone());
                let refresh = use_signal(|| 0);
                let other_source = use_pending_commands(refresh, |_| false);
                let entity = delivery_command_entity(&fixture.initial.detail.delivery.id);
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
        let form = open_reviewed_proof(&mut page);
        click_button(&mut page, "列表提交同一投递");
        assert!(has_live_text(&page, "这条投递还有未了结的操作，请先在未决区核对。"));
        assert!(has_live_text(&page, "准入通过。确认时仍会重新核验快照。"));
        assert!(button(&page, "确认已处置").unwrap().1);
        assert_eq!(submit_button_disabled(&page, "确认证明"), Some(true));
        page.dispatch("submit", form, "");
        assert_eq!(confirms.started(), 0);
        assert!(button(&page, "打开人工 Sent 证明").unwrap().1);
        click_button(&mut page, "取消");
        let (sent, _) = button(&page, "打开人工 Sent 证明").unwrap();
        page.dispatch("click", sent, "");
        assert!(submit_button_disabled(&page, "确认证明").is_none());
    }

    #[test]
    fn sent_proof_receipt_survives_the_failed_delivery_read() {
        let fixture = fixture();
        let requests = fixture.requests.clone();
        let delivery_id = fixture.initial.detail.delivery.id.clone();
        let detail_url = format!("/api/v1/admin/deliveries/{delivery_id}?include=sent_proof");
        let confirms = Rc::new(RecordRead::new());
        let (mut page, _) = mount_writer({
            let confirms = confirms.clone();
            move || {
                init_i18n();
                use_preview_stub();
                use_detail_fixture(&fixture);
                use_context_provider(|| confirms.clone());
                use_context_provider(|| {
                    let failure = AdminApiFailure::from_http_body(
                        503,
                        r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
                    );
                    DetailGetFailure { request_number: 2, code: failure.code }
                });
                rsx! { Router::<TestRoute> {} }
            }
        });
        assert_eq!(requests.borrow().as_slice(), [detail_url.as_str()]);
        let form = open_reviewed_proof(&mut page);
        page.dispatch("submit", form, "");
        assert_eq!(requests.borrow().as_slice(), [detail_url.as_str()]);
        confirms.respond(Ok(DeliverySentProofResult {
            command_id: uuid::Uuid::from_bytes([1; 16]).to_string(),
            delivery_id,
            external_reference: "mooglemail:main:send:7000001".to_owned(),
            replayed: false,
        }));
        settle_writer(&mut page);
        assert_eq!(requests.borrow().as_slice(), [detail_url.as_str(); 2]);
        assert!(has_live_text(&page, "所需服务暂时不可用。请稍后再试。"));
        let receipt = page.dom().in_scope(dioxus::core::ScopeId::APP, || {
            auction_web_shared::t!("delivery-sent-proof-submitted")
        });
        assert!(has_live_text(&page, &receipt));
        assert!(button(&page, "打开人工 Sent 证明").is_none());
        click_button(&mut page, "重试");
        assert_eq!(requests.borrow().as_slice(), [detail_url.as_str(); 3]);
        assert_eq!(confirms.started(), 1);
        assert!(!has_live_text(&page, "所需服务暂时不可用。请稍后再试。"));
        assert!(has_live_text(&page, &receipt));
    }

    #[test]
    fn stale_fence_on_confirmation_reloads_detail_and_removes_old_evidence() {
        const DETAIL_GET: &str =
            "/api/v1/admin/deliveries/0f8fad5b-d9cb-469f-a165-70867728950e?include=sent_proof";
        const CONFIRM_POST: &str =
            "/api/v1/admin/deliveries/0f8fad5b-d9cb-469f-a165-70867728950e/sent-proof/confirm";
        let [mut projection, _, _]: [Value; 3] = serde_json::from_str(include_str!(
            "../../../../../auction-contracts/tests/fixtures/admin/delivery-sent-proof-detail.json"
        ))
        .unwrap();
        let initial: AdminDeliveryDetailWithActionsResponse =
            serde_json::from_value(projection["data"].take()).unwrap();
        let old_reference = initial.detail.evidence[0].reference.clone();
        let mut refreshed = initial.clone();
        refreshed.detail.delivery.status = "failed".to_owned();
        refreshed.detail.delivery.fencing_version += 1;
        refreshed.detail.evidence.clear();
        let requests = Rc::new(RefCell::new(Vec::new()));
        let fixture =
            DetailFixture { initial, refreshed: Some(refreshed), requests: requests.clone() };
        let (mut page, _) = mount_writer(move || {
            init_i18n();
            use_preview_stub();
            use_detail_fixture(&fixture);
            AdminApiFailure::stub_write_response(
                CONFIRM_POST,
                Method::POST,
                409,
                r#"{"code":"stale_delivery_state","message":"投递状态已变化","data":null,"request_id":null}"#,
            );
            rsx! { Router::<TestRoute> {} }
        });
        assert_eq!(*requests.borrow(), [DETAIL_GET]);
        assert!(has_live_text(&page, &old_reference));
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
        assert_eq!(submit_button_disabled(&page, "确认证明"), Some(false));
        assert_eq!(*requests.borrow(), [DETAIL_GET]);
        page.dispatch("submit", form, "");

        assert_eq!(*requests.borrow(), [DETAIL_GET, DETAIL_GET]);
        assert!(!has_live_text(&page, &old_reference));
        assert!(button(&page, "打开人工 Sent 证明").is_none());
        assert!(submit_button_disabled(&page, "确认证明").is_none());
        assert!(has_live_text(
            &page,
            "投递状态或证据已变化，请核对最新详情后重试。提交 Sent 证明时请重新选择证据。"
        ));
    }
}
