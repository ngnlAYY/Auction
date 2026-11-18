//! Admin 拍卖生命周期动作按钮与确认对话框。

use auction_web_shared::{Dialog, t, use_second_tick};
use dioxus::prelude::*;

use super::super::editor::{
    DraftFeedback, DraftRead, DraftRefreshButton, close_edit_draft, close_matching_draft,
    focus_draft_field_after_render, observe_draft_write,
};
use super::super::presentation::current_time_millis;
use super::AuctionDrafts;
use super::model::{
    AdminAuctionNotice, LifecycleReasonIssue, lifecycle_action_button_class,
    lifecycle_action_confirm_note, lifecycle_action_confirm_title, lifecycle_action_label,
    lifecycle_action_submit_label, lifecycle_actions, pending_open_millis, stale_auction_failure,
    validate_lifecycle_reason,
};
use crate::api::{
    AdminAuction, AdminAuctionCapabilities, AdminAuctionLifecycleAction,
    execute_admin_auction_lifecycle,
};
use crate::surface::admin_world_label;
use crate::surface::pending::{PendingCommandPanel, PendingCommands, set_if_alive};
use crate::surface::router::Route;
use crate::surface::validation::FieldError;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct LifecycleTarget {
    auction_id: String,
    world_id: String,
    sequence: i64,
    state_version: i64,
}

impl From<&AdminAuction> for LifecycleTarget {
    fn from(auction: &AdminAuction) -> Self {
        Self {
            auction_id: auction.id.clone(),
            world_id: auction.world_id.clone(),
            sequence: auction.sequence,
            state_version: auction.state_version,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct LifecycleDraft {
    target: LifecycleTarget,
    action: AdminAuctionLifecycleAction,
    reason: String,
}

impl LifecycleDraft {
    pub(super) fn new(target: LifecycleTarget, action: AdminAuctionLifecycleAction) -> Self {
        Self { target, action, reason: String::new() }
    }

    pub(super) fn auction_id(&self) -> &str {
        &self.target.auction_id
    }
}

#[component]
pub(super) fn AdminAuctionActions(
    auction: AdminAuction,
    capabilities: AdminAuctionCapabilities,
    notice: Signal<Option<AdminAuctionNotice>>,
    edits_ready: bool,
) -> Element {
    // 只有预览阶段等待开拍的入口订阅秒级时钟。
    let now = current_time_millis();
    let owner = use_context::<PendingCommands>();
    let entity = format!("auction:{}", auction.id);
    let blocked = !edits_ready || owner.contains(&entity);
    let drafts = use_context::<AuctionDrafts>();
    let target = LifecycleTarget::from(&auction);
    let actions =
        lifecycle_actions(&auction.status, &capabilities, &auction.scheduled_open_at, now);
    let waiting_until = pending_open_millis(
        &auction.status,
        capabilities.manual_early_open,
        &auction.scheduled_open_at,
        now,
    );
    let can_edit_schedule =
        capabilities.modify_schedule && matches!(auction.status.as_str(), "draft" | "preview");
    rsx! {
        div { class: "admin-auctions__actions",
            Link {
                class: "admin-auctions__open-link",
                to: Route::AuctionLots { auction_id: auction.id.clone() },
                {t!("admin-auctions-open")}
            }
            if let Some(scheduled_open_millis) = waiting_until {
                AdminAuctionScheduledOpenButton {
                    scheduled_open_millis,
                    blocked,
                    notice,
                    target: target.clone(),
                    drafts,
                }
            }
            for action in actions.iter().copied() {
                AdminAuctionActionButton {
                    action,
                    blocked,
                    notice,
                    target: target.clone(),
                    drafts,
                }
            }
            if can_edit_schedule {
                button {
                    class: "admin-btn-secondary",
                    r#type: "button",
                    disabled: blocked,
                    onclick: move |_| {
                        if !blocked && !owner.contains(&entity) && drafts.open_schedule(&auction) {
                            let mut notice = notice;
                            notice.set(None);
                        }
                    },
                    {t!("admin-auctions-action-schedule")}
                }
            }
        }
    }
}

/// 等待开拍时间的「开拍」入口，仍排在其他生命周期动作之前。
#[component]
fn AdminAuctionScheduledOpenButton(
    scheduled_open_millis: i64,
    blocked: bool,
    notice: Signal<Option<AdminAuctionNotice>>,
    target: LifecycleTarget,
    drafts: AuctionDrafts,
) -> Element {
    let second_tick = use_second_tick();
    let _ = second_tick();
    if current_time_millis() < scheduled_open_millis {
        return rsx! {};
    }
    rsx! {
        AdminAuctionActionButton {
            action: AdminAuctionLifecycleAction::Open,
            blocked,
            notice,
            target,
            drafts,
        }
    }
}

#[component]
fn AdminAuctionActionButton(
    action: AdminAuctionLifecycleAction,
    blocked: bool,
    mut notice: Signal<Option<AdminAuctionNotice>>,
    target: LifecycleTarget,
    drafts: AuctionDrafts,
) -> Element {
    rsx! {
        button {
            class: lifecycle_action_button_class(action),
            r#type: "button",
            disabled: blocked,
            onclick: move |_| {
                if !blocked && drafts.open_lifecycle(target.clone(), action) {
                    notice.set(None);
                }
            },
            {lifecycle_action_label(action)}
        }
    }
}

#[component]
pub(super) fn AdminAuctionActionDialog(
    latest_version: Option<i64>,
    notice: Signal<Option<AdminAuctionNotice>>,
    mut pending: Signal<Option<LifecycleDraft>>,
    reload_generation: Signal<u64>,
    read: DraftRead,
) -> Element {
    let owner = use_context::<PendingCommands>();
    let mut issue = use_signal(|| None::<LifecycleReasonIssue>);
    let mut feedback = use_signal(|| None::<String>);
    let Some(draft) = pending() else {
        return rsx! {};
    };
    let action = draft.action;
    let is_cancel = action == AdminAuctionLifecycleAction::Cancel;
    let requires_reason = is_cancel || action == AdminAuctionLifecycleAction::EarlySettlement;
    let auction_id = draft.target.auction_id.clone();
    let entity = format!("auction:{auction_id}");
    let busy = owner.is_busy(&entity);
    let blocked = owner.contains(&entity);
    let error = feedback();
    let conflict = !blocked
        && (error.as_deref().is_some_and(stale_auction_failure)
            || read.version_changed(draft.target.state_version, latest_version));
    let can_submit = read.is_ready()
        && !blocked
        && !conflict
        && latest_version == Some(draft.target.state_version);
    let on_close = move |()| close_edit_draft(pending, reload_generation, conflict);
    let action_reason_id = format!("action-reason-{auction_id}");
    let focus_id = action_reason_id.clone();
    let hint_id = format!("{action_reason_id}-hint");
    let error_id = format!("{action_reason_id}-error");
    let described_by =
        if issue().is_some() { format!("{hint_id} {error_id}") } else { hint_id.clone() };
    let world = admin_world_label(&draft.target.world_id);
    let sequence = draft.target.sequence;

    rsx! {
        Dialog {
            title: lifecycle_action_confirm_title(action, sequence, &world),
            close_label: t!("admin-auctions-action-close"),
            busy,
            on_close,
            form {
                class: "admin-auction-open",
                novalidate: true,
                onsubmit: move |event| {
                    event.prevent_default();
                    if !can_submit { return; }
                    let Some(current) = pending() else { return; };
                    let reason_value = if requires_reason {
                        match validate_lifecycle_reason(&current.reason) {
                            Ok(value) => Some(value.to_owned()),
                            Err(error) => {
                                issue.set(Some(error));
                                focus_draft_field_after_render(focus_id.clone());
                                return;
                            }
                        }
                    } else {
                        None
                    };
                    issue.set(None);
                    if run_lifecycle_action(owner, current, reason_value, feedback, notice, pending) {
                        feedback.set(None);
                    }
                },
                p { {t!("admin-auctions-confirm-target", world: world, number: sequence)} }
                p { {lifecycle_action_confirm_note(action)} }
                if requires_reason {
                    label {
                        r#for: action_reason_id.clone(),
                        if is_cancel { {t!("admin-auctions-cancel-reason")} }
                        else { {t!("admin-auctions-early-settlement-reason")} }
                    }
                    textarea {
                        id: action_reason_id,
                        name: "reason",
                        required: true,
                        autofocus: !conflict,
                        disabled: blocked,
                        readonly: conflict,
                        value: draft.reason,
                        aria_invalid: issue().is_some().to_string(),
                        aria_describedby: described_by,
                        oninput: move |event| {
                            if blocked || conflict { return; }
                            if let Some(draft) = pending.write().as_mut() {
                                draft.reason = event.value();
                            }
                            issue.set(None);
                        },
                    }
                    p { id: hint_id,
                        if is_cancel { {t!("admin-auctions-cancel-reason-hint")} }
                        else { {t!("admin-auctions-early-settlement-reason-hint")} }
                    }
                    FieldError { id: error_id, message: issue().map(LifecycleReasonIssue::message) }
                }
                if !blocked {
                    DraftFeedback { conflict, read: read.clone(), failure: error }
                }
                div { class: "admin-shell__dialog-actions",
                    if !conflict {
                        button {
                            r#type: "button",
                            disabled: busy,
                            autofocus: !requires_reason,
                            onclick: move |_| on_close(()),
                            {t!("admin-auctions-action-close")}
                        }
                    }
                    if !blocked {
                        DraftRefreshButton { read, conflict, reload: reload_generation, on_close }
                    }
                    if !conflict && (!blocked || busy) {
                        button {
                            class: lifecycle_action_button_class(action),
                            r#type: "submit",
                            disabled: !can_submit,
                            if busy { {t!("admin-action-working")} }
                            else { {lifecycle_action_submit_label(action)} }
                        }
                    }
                }
            }
            if blocked && !busy {
                PendingCommandPanel { owner }
            }
        }
    }
}

/// 页级请求以操作员打开确认时看到的版本提交，未知结果只重试同一完整意图。
fn run_lifecycle_action(
    owner: PendingCommands,
    draft: LifecycleDraft,
    reason: Option<String>,
    feedback: Signal<Option<String>>,
    notice: Signal<Option<AdminAuctionNotice>>,
    pending: Signal<Option<LifecycleDraft>>,
) -> bool {
    let action = draft.action;
    let submitted_target = draft.target.clone();
    owner.submit(
        format!("auction:{}", draft.target.auction_id),
        lifecycle_action_label(action),
        move |command_id| {
            observe_draft_write(
                execute_admin_auction_lifecycle(
                    command_id,
                    draft.target.auction_id.clone(),
                    action,
                    draft.target.state_version,
                    reason.clone(),
                ),
                feedback,
            )
        },
        move |_| {
            set_if_alive(feedback, None);
            // 未决区回执只关闭同一个版本的确认，不能影响另一个场次的草稿。
            close_matching_draft(pending, |draft| {
                draft.target == submitted_target && draft.action == action
            });
            set_if_alive(notice, Some(AdminAuctionNotice::Lifecycle(action)));
        },
        |_| {},
    )
}

#[cfg(test)]
mod tests {
    use auction_web_shared::i18n::{DEFAULT_ADMIN_LOCALE, build_bundle};
    use dioxus::dioxus_core::Mutation;
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::{AdminAuctionActionDialog, LifecycleDraft, LifecycleTarget};
    use crate::api::AdminAuctionLifecycleAction;
    use crate::surface::ADMIN_LOCALE_CATALOG;
    use crate::surface::auctions::editor::DraftRead;
    use crate::surface::pending::{use_pending_command_host, use_pending_commands};

    fn final_dialog(world_id: String) -> Element {
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_ADMIN_LOCALE,
                ADMIN_LOCALE_CATALOG.default_ftl,
                &DEFAULT_ADMIN_LOCALE,
                None,
            )
        })
        .unwrap();
        let reload_generation = use_signal(|| 0);
        use_pending_command_host();
        use_pending_commands(reload_generation, |_| true);
        let notice = use_signal(|| None);
        let target = LifecycleTarget {
            auction_id: "test-auction".to_owned(),
            world_id,
            sequence: 74,
            state_version: 5,
        };
        let pending = use_signal(|| {
            Some(LifecycleDraft::new(target.clone(), AdminAuctionLifecycleAction::FinalPhase))
        });
        rsx! { AdminAuctionActionDialog { latest_version: Some(5), notice, pending, reload_generation, read: DraftRead::Ready } }
    }

    #[test]
    fn final_confirmation_names_the_world_and_round_without_assuming_minutes() {
        for (world, label) in [("main", "主世界"), ("isekai", "Isekai")] {
            let edits =
                VirtualDom::new_with_props(final_dialog, world.to_owned()).rebuild_to_vec().edits;
            let text = edits
                .iter()
                .filter_map(|edit| match edit {
                    Mutation::CreateTextNode { value, .. } => Some(value.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                text.iter().filter(|value| value.contains(label) && value.contains("74")).count(),
                2
            );
            assert!(text.iter().any(|value| value.contains("本场冻结的策略")));
            assert!(text.iter().all(|value| !value.contains("15 分钟")));
        }
    }
}
