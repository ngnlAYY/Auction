//! 结算退件详情页的处理动作按服务端允许的动作显示按钮，确认后交给会话命令宿主。
//!
//! 确认框只展示服务端冻结的后果（原收件人、资产与补寄次数），不提供可编辑的资产、金额或收件人。
//! 提交时冻结案件、版本、动作与 `command_id`。结果未知可在未决区原样重试，被明确拒绝时重读详情。

use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;

use super::labels::{action_effect_key, action_key, blocker_key, label};
use super::list::assets_text;
use crate::api::{
    SettlementReturnDetail, SettlementReturnSummary, submit_settlement_return_action,
};
use crate::surface::AdminActionFeedback;
use crate::surface::pending::{PendingCommands, definite_rejection, set_if_alive};

/// 一次待确认的动作：冻结读取时的案件版本，确认后原样提交。
#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingAction {
    action: String,
    expected_version: i64,
}

#[component]
pub(super) fn ActionSection(
    detail: SettlementReturnDetail,
    notice: Signal<Option<AdminActionFeedback>>,
    refresh: Signal<u64>,
    loaded_generation: u64,
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
            if let Some(feedback) = notice() {
                p {
                    class: if feedback.is_success() {
                        "action-feedback action-feedback--success"
                    } else {
                        "action-feedback action-feedback--error"
                    },
                    role: if feedback.is_success() { "status" } else { "alert" },
                    {feedback.message(t!("settlement-return-action-success"))}
                }
            }
            if let Some(pending) = confirming() {
                ConfirmDialog { summary, pending, confirming, notice, refresh, loaded_generation }
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
) -> Element {
    let commands = use_context::<PendingCommands>();
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
                    .map_err(|failure| failure.code)
                }
            },
            move |_| set_if_alive(notice, Some(AdminActionFeedback::Success)),
            move |code| {
                // 明确拒绝（版本已变、状态或资格不符）说明案件已不同于页面所见：重读后再决定；
                // 结果未知不重读，原 command_id 留在未决区原样重试。
                if definite_rejection(&code) {
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
    use crate::surface::pending::definite_rejection;

    /// 结算退件命令的类型化拒绝都确定没有写入，页面据此重读详情；结果未知（网络、超时、5xx、
    /// 无效响应）不是确定拒绝，原 command_id 留在未决区原样重试。
    #[test]
    fn typed_rejections_are_definite_and_unknown_outcomes_are_not() {
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
            assert!(definite_rejection(code), "{code} 应是确定拒绝");
        }
        for code in [
            "temporarily_unavailable",
            "network_error",
            "timeout",
            "invalid_response",
            "http_503",
            "outcome_unknown",
        ] {
            assert!(!definite_rejection(code), "{code} 结果未知，不能当作确定拒绝");
        }
    }
}
