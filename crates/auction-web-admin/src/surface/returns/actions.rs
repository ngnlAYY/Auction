//! 结算退件详情页的处理动作按服务端允许的动作显示按钮，确认后交给会话命令宿主。
//!
//! 确认框只展示服务端冻结的后果（原收件人、资产与补寄次数），不提供可编辑的资产、金额或收件人。
//! 提交时冻结案件、版本、动作与 `command_id`。结果未知可在未决区原样重试，被明确拒绝时重读详情。

use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;

use super::labels::{action_effect_key, action_key, blocker_key, label};
use super::list::assets_text;
use crate::api::{
    SettlementReturnDetail, SettlementReturnSummary, return_failure_needs_reload,
    submit_settlement_return_action,
};
use crate::surface::AdminActionFeedback;
use crate::surface::pending::{PendingCommands, set_if_alive};

/// 一次待确认的动作：冻结读取时的案件版本，确认后原样提交。
#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingAction {
    action: String,
    expected_version: i64,
}

/// 详情页的动作入口、阻断原因与确认框。
#[component]
pub(super) fn ActionSection(
    detail: SettlementReturnDetail,
    notice: Signal<Option<AdminActionFeedback>>,
    refresh: Signal<u64>,
    loaded_generation: u64,
    restore_focus: EventHandler<()>,
) -> Element {
    let commands = use_context::<PendingCommands>();
    let mut confirming = use_signal(|| None::<PendingAction>);
    let SettlementReturnDetail { summary, allowed_actions, blockers, .. } = detail;
    let entity = format!("settlement-return:{}", summary.return_id);
    let can_open = use_callback(move |()| {
        refresh() == loaded_generation && !commands.contains(&entity) && !commands.any_busy()
    });
    let version = summary.case_version;
    // 按钮文案在渲染前取好，按钮只捕获动作名与读取时的案件版本。
    let buttons: Vec<(String, String)> = allowed_actions
        .into_iter()
        .map(|action| {
            let text = label(action_key, &action);
            (action, text)
        })
        .collect();
    let blockers: Vec<String> = blockers
        .iter()
        .map(|item| {
            format!("{}：{}", label(action_key, &item.action), label(blocker_key, &item.blocker))
        })
        .collect();
    rsx! {
        section { class: "admin-panel admin-settlement-return-actions",
            aria_labelledby: "settlement-return-actions-title",
            h2 { id: "settlement-return-actions-title", {t!("settlement-return-actions-title")} }
            if buttons.is_empty() {
                p { class: "admin-panel__note", {t!("settlement-return-actions-none")} }
            } else {
                div { class: "admin-settlement-return-actions__buttons",
                    for (action, text) in buttons {
                        button {
                            key: "{action}",
                            class: "admin-btn-primary",
                            r#type: "button",
                            disabled: !can_open.call(()),
                            onclick: move |_| {
                                if !can_open.call(()) { return; }
                                let mut notice = notice;
                                notice.set(None);
                                confirming.set(Some(PendingAction {
                                    action: action.clone(),
                                    expected_version: version,
                                }));
                            },
                            "{text}"
                        }
                    }
                }
            }
            if !blockers.is_empty() {
                h3 { {t!("settlement-return-blocked-title")} }
                ul { class: "admin-settlement-return-actions__blockers",
                    for blocker in blockers {
                        li { "{blocker}" }
                    }
                }
            }
            if let Some(pending) = confirming() {
                ConfirmDialog { summary, pending, confirming, notice, refresh, loaded_generation, restore_focus }
            }
        }
    }
}

#[component]
fn ConfirmDialog(
    summary: SettlementReturnSummary,
    pending: PendingAction,
    confirming: Signal<Option<PendingAction>>,
    notice: Signal<Option<AdminActionFeedback>>,
    refresh: Signal<u64>,
    loaded_generation: u64,
    restore_focus: EventHandler<()>,
) -> Element {
    let commands = use_context::<PendingCommands>();
    use_drop(move || restore_focus.call(()));
    let recipient = summary.recipient.clone().unwrap_or_else(|| t!("value-empty"));
    let used = summary.resends_used.unwrap_or_default();
    let effect = action_effect_key(&pending.action).map_or_else(
        || pending.action.clone(),
        |key| {
            t!(
                key,
                recipient: recipient.clone(),
                used: used.to_string(),
                limit: summary.resend_limit.to_string()
            )
        },
    );
    let title = label(action_key, &pending.action);
    let expected_version = pending.expected_version;
    let return_id = summary.return_id.clone();
    let same_version = expected_version == summary.case_version;
    // 确认绑定打开时的案件版本，读取世代切换后旧点击不能分配新的命令 ID。
    let can_submit = move || same_version && refresh() == loaded_generation && !commands.any_busy();
    let blocked = !can_submit() || commands.contains(&format!("settlement-return:{return_id}"));
    let assets = assets_text(&summary);
    let submit = move |_| {
        let mut confirming = confirming;
        let mut notice = notice;
        if !can_submit() || commands.contains(&format!("settlement-return:{return_id}")) {
            return;
        }
        let intent = pending.clone();
        let entity = return_id.clone();
        let submitted = commands.submit(
            format!("settlement-return:{entity}"),
            label(action_key, &intent.action),
            move |command_id| {
                let return_id = entity.clone();
                let intent = intent.clone();
                async move {
                    submit_settlement_return_action(
                        command_id,
                        return_id,
                        intent.expected_version,
                        intent.action,
                    )
                    .await
                }
            },
            move |_| set_if_alive(notice, Some(AdminActionFeedback::Success)),
            move |code| {
                // 保留显示码原有的重读条件；会话宿主独立裁定原 command_id 是否仍未决。
                if return_failure_needs_reload(&code) {
                    let mut refresh = refresh;
                    if let Ok(mut value) = refresh.try_write() {
                        *value = value.saturating_add(1);
                    }
                }
                set_if_alive(notice, Some(AdminActionFeedback::Error(code)));
            },
        );
        if submitted {
            confirming.set(None);
            notice.set(None);
        }
    };
    rsx! {
        Dialog {
            title: title,
            close_label: t!("settlement-return-cancel"),
            busy: commands.any_busy(),
            on_close: move |_| {
                let mut confirming = confirming;
                confirming.set(None);
            },
            p { "{effect}" }
            dl { class: "admin-delivery-detail__grid",
                dt { {t!("settlement-return-column-recipient")} }
                dd { "{recipient}" }
                dt { {t!("settlement-return-column-assets")} }
                dd { "{assets}" }
                dt { {t!("settlement-return-field-version")} }
                dd { "{expected_version}" }
            }
            div { class: "admin-shell__dialog-actions",
                button {
                    class: "admin-btn-secondary",
                    r#type: "button",
                    disabled: commands.any_busy(),
                    autofocus: true,
                    onclick: move |_| {
                        let mut confirming = confirming;
                        confirming.set(None);
                    },
                    {t!("settlement-return-cancel")}
                }
                button {
                    class: "admin-btn-primary",
                    r#type: "button",
                    disabled: blocked,
                    onclick: submit,
                    {t!("settlement-return-confirm")}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use dioxus::prelude::{
        Element, ReadableExt, Signal, WritableExt, component, dioxus_core, dioxus_elements,
        dioxus_signals, rsx, use_context, use_context_provider, use_signal,
    };
    use gloo_net::http::Method;
    use serde_json::Value;
    use uuid::Uuid;

    use super::{ActionSection, SettlementReturnDetail};
    use crate::api::{
        AdminApiFailure, AdminWriteEndpoint, AdminWriteFailure, return_failure_needs_reload,
    };
    use crate::surface::auctions::draft_page_tests::{
        RecordRead, button, click_button, command_button, has_live_text, has_text, mount_writer,
        settle_writer,
    };
    use crate::surface::pending::{
        PendingCommandNotices, PendingCommandPanel, use_pending_command_host, use_pending_commands,
    };

    const CASE: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
    const HANDLE: &str =
        "/api/v1/admin/settlement-returns/0f8fad5b-d9cb-469f-a165-70867728950e/handle";

    fn return_detail() -> SettlementReturnDetail {
        let mut body: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../auction-contracts/tests/fixtures/admin/settlement-return.json"
        )))
        .unwrap();
        body["data"]["summary"]["return_id"] = Value::String(CASE.to_owned());
        body["data"]["summary"]["case_version"] = Value::from(2);
        body["data"]["allowed_actions"] = serde_json::json!(["reclaim_and_resend"]);
        serde_json::from_value(body["data"].take()).unwrap()
    }

    // 原用例覆盖的显示码继续逐一检查，以详情是否需要重读为期望。
    #[test]
    fn only_return_reload_display_codes_request_an_extra_read() {
        for code in [
            "invalid_settlement_return_command",
            "invalid_settlement_return_id",
            "settlement_return_not_found",
            "stale_settlement_return_state",
            "command_conflict",
            "settlement_return_untrusted",
            "settlement_return_unbound",
            "settlement_return_assets_mismatch",
            "settlement_return_action_conflict",
            "settlement_return_proof_required",
            "settlement_return_resend_limit",
            "settlement_return_obligation_ineligible",
            "settlement_return_send_unresolved",
            "settlement_return_observation_required",
        ] {
            assert!(return_failure_needs_reload(code), "{code} 应重读案件");
        }
        for code in [
            "temporarily_unavailable",
            "network_error",
            "timeout",
            "invalid_response",
            "http_503",
            "outcome_unknown",
        ] {
            assert!(!return_failure_needs_reload(code), "{code} 不应触发退件额外重读");
        }
    }

    #[test]
    fn return_write_receipt_refreshes_detail_only_for_matching_display_codes() {
        for (status, body, expected_reads, pending) in [
            (
                409,
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../auction-contracts/tests/fixtures/admin/error-stale-settlement-return-state.json"
                )),
                1,
                false,
            ),
            (
                503,
                r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
                0,
                true,
            ),
        ] {
            let generation = Rc::new(Cell::<Option<Signal<u64>>>::new(None));
            let restored = Rc::new(Cell::new(0));
            let (mut page, _) = mount_writer({
                let generation = generation.clone();
                let restored = restored.clone();
                move || {
                    let detail = return_detail();
                    let refresh = use_signal(|| 0);
                    generation.set(Some(refresh));
                    let notice = use_signal(|| None);
                    // 单独观察退件自己的重读；宿主了结通知由另一份读取代际接收。
                    let owner = use_pending_commands(use_signal(|| 0), |_| true);
                    AdminApiFailure::stub_write_response(HANDLE, Method::POST, status, body);
                    let restored = restored.clone();
                    rsx! {
                        PendingCommandPanel { owner }
                        ActionSection {
                            detail, notice, refresh, loaded_generation: 0,
                            restore_focus: move |()| restored.set(restored.get() + 1),
                        }
                    }
                }
            });
            click_button(&mut page, "领回并补寄");
            click_button(&mut page, "取消");
            assert_eq!(restored.get(), 1);
            assert_eq!(*generation.get().unwrap().peek(), 0);
            click_button(&mut page, "领回并补寄");
            click_button(&mut page, "确认提交");
            assert_eq!(restored.get(), 2);
            assert_eq!(*generation.get().unwrap().peek(), expected_reads);
            assert_eq!(button(&page, "原样重试").is_some(), pending);
        }
    }

    struct ReturnAttempts {
        command_ids: RefCell<Vec<Uuid>>,
        reply: RecordRead<(), AdminWriteFailure>,
    }

    #[component]
    fn ReturnActionSource() -> Element {
        let attempts = use_context::<Rc<ReturnAttempts>>();
        let refresh = use_signal(|| 0);
        let notice = use_signal(|| None);
        let target = format!("settlement-return:{CASE}");
        let owner = use_pending_commands(refresh, {
            let target = target.clone();
            move |entity| entity == target
        });
        rsx! {
            // 用可延迟的执行体模拟已确认的动作；确认框的接线由另一条用例覆盖。
            button { onclick: move |_| {
                let attempts = attempts.clone();
                assert!(owner.submit(
                    target.clone(), "领回并补寄".to_owned(),
                    move |command_id| {
                        let attempts = attempts.clone();
                        async move {
                            attempts.command_ids.borrow_mut().push(command_id);
                            attempts.reply.fetch().await
                        }
                    },
                    |()| panic!("成功回执应交给外壳"),
                    |_| panic!("坏回执到达时来源详情应已卸载"),
                ));
            }, {"提交已确认的退件操作".to_owned()} }
            PendingCommandPanel { owner }
            ActionSection {
                detail: return_detail(), notice, refresh, loaded_generation: refresh(),
                restore_focus: |_| {},
            }
        }
    }

    #[test]
    fn unreadable_return_receipt_stays_retryable_in_shell_after_detail_unmounts() {
        let attempts = Rc::new(ReturnAttempts {
            command_ids: RefCell::new(Vec::new()),
            reply: RecordRead::new(),
        });
        let (mut page, _) = mount_writer({
            let attempts = attempts.clone();
            move || {
                use_context_provider(|| attempts.clone());
                let host = use_pending_command_host();
                let mut visible = use_signal(|| true);
                rsx! {
                    button { onclick: move |_| visible.toggle(), {"切换退件详情".to_owned()} }
                    if visible() { ReturnActionSource {} }
                    PendingCommandNotices { host }
                }
            }
        });
        click_button(&mut page, "提交已确认的退件操作");
        let [command_id] = attempts.command_ids.borrow().as_slice().try_into().unwrap();
        click_button(&mut page, "切换退件详情");
        assert!(!has_live_text(&page, "处理"));
        assert!(button(&page, "原样重试").is_none());

        attempts.reply.respond(Err(AdminWriteFailure::from_test_http(
            AdminWriteEndpoint::General,
            409,
            "{",
        )));
        settle_writer(&mut page);
        assert!(command_button(&page, command_id, "原样重试").is_some());
        click_button(&mut page, "切换退件详情");
        assert!(has_live_text(&page, "处理"));
        assert!(button(&page, "领回并补寄").unwrap().1);
        click_button(&mut page, "切换退件详情");
        assert!(!has_live_text(&page, "处理"));
        assert_eq!(attempts.command_ids.borrow().as_slice(), [command_id]);

        let (retry, false) = command_button(&page, command_id, "原样重试").expect("原命令重试按钮")
        else {
            panic!("原命令必须可重试");
        };
        page.dispatch("click", retry, "");
        assert_eq!(
            attempts.command_ids.borrow().as_slice(),
            [command_id, command_id],
            "点击外壳重试应再次执行同一命令",
        );
        assert!(command_button(&page, command_id, "原样重试").unwrap().1);
        attempts.reply.respond(Ok(()));
        let settled = settle_writer(&mut page);
        assert!(button(&page, "原样重试").is_none());
        assert!(has_text(&settled, "已成功。"));
        click_button(&mut page, "关闭回执");
        assert!(button(&page, "关闭回执").is_none());
        click_button(&mut page, "切换退件详情");
        assert!(!button(&page, "领回并补寄").unwrap().1);
        assert_eq!(attempts.command_ids.borrow().as_slice(), [command_id, command_id]);
    }
}
