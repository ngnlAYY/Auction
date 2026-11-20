//! Admin 拍卖生命周期动作的闭集模型与文案映射。

use auction_web_shared::t;

use super::super::presentation::scheduled_time_millis;
use crate::api::{AdminAuctionCapabilities, AdminAuctionLifecycleAction};

/// 拍卖列表顶部一次只显示一个成功动作，避免各行刷新后残留局部提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminAuctionNotice {
    Lifecycle(AdminAuctionLifecycleAction),
    Schedule,
    Created,
}

pub(super) fn lifecycle_actions(
    status: &str,
    capabilities: &AdminAuctionCapabilities,
    scheduled_open_at: &str,
    current_time_millis: i64,
) -> Vec<AdminAuctionLifecycleAction> {
    let mut actions = Vec::new();
    match status {
        "draft" => {
            if capabilities.manual_preview_open {
                actions.push(AdminAuctionLifecycleAction::Preview);
            }
            if capabilities.cancel_auction {
                actions.push(AdminAuctionLifecycleAction::Cancel);
            }
        }
        "preview" => {
            if capabilities.manual_early_open
                || scheduled_time_millis(scheduled_open_at)
                    .is_some_and(|scheduled| scheduled <= current_time_millis)
            {
                actions.push(AdminAuctionLifecycleAction::Open);
            }
            if capabilities.cancel_auction {
                actions.push(AdminAuctionLifecycleAction::Cancel);
            }
        }
        "live" => {
            actions.push(AdminAuctionLifecycleAction::FinalPhase);
            if capabilities.manual_early_settlement {
                actions.push(AdminAuctionLifecycleAction::EarlySettlement);
            }
        }
        "final" if capabilities.manual_early_settlement => {
            actions.push(AdminAuctionLifecycleAction::EarlySettlement);
        }
        _ => {}
    }
    actions
}

/// 预览场次未开启提前开拍、且开拍时间尚未到达时返回开拍时刻。只有这类行的就绪标记与
/// “开拍”入口会随时间变化，因而只有它们挂载秒级时钟；其余状态的输出与时间无关。
pub(super) fn pending_open_millis(
    status: &str,
    manual_early_open: bool,
    scheduled_open_at: &str,
    current_time_millis: i64,
) -> Option<i64> {
    if status != "preview" || manual_early_open {
        return None;
    }
    scheduled_time_millis(scheduled_open_at).filter(|scheduled| *scheduled > current_time_millis)
}

pub(super) fn lifecycle_action_label(action: AdminAuctionLifecycleAction) -> String {
    match action {
        AdminAuctionLifecycleAction::Preview => t!("admin-auctions-action-preview"),
        AdminAuctionLifecycleAction::Open => t!("admin-auctions-action-open"),
        AdminAuctionLifecycleAction::Cancel => t!("admin-auctions-action-cancel"),
        AdminAuctionLifecycleAction::FinalPhase => t!("admin-auctions-action-final"),
        AdminAuctionLifecycleAction::EarlySettlement => {
            t!("admin-auctions-action-early-settlement")
        }
    }
}

pub(super) fn lifecycle_action_button_class(action: AdminAuctionLifecycleAction) -> &'static str {
    match action {
        AdminAuctionLifecycleAction::Preview | AdminAuctionLifecycleAction::Open => {
            "admin-btn-primary"
        }
        AdminAuctionLifecycleAction::Cancel
        | AdminAuctionLifecycleAction::FinalPhase
        | AdminAuctionLifecycleAction::EarlySettlement => "admin-btn-danger",
    }
}

pub(super) fn lifecycle_action_submit_label(action: AdminAuctionLifecycleAction) -> String {
    match action {
        AdminAuctionLifecycleAction::FinalPhase => t!("admin-auctions-action-confirm"),
        AdminAuctionLifecycleAction::Cancel => t!("admin-auctions-action-cancel-confirm"),
        AdminAuctionLifecycleAction::EarlySettlement => {
            t!("admin-auctions-action-early-settlement-confirm")
        }
        AdminAuctionLifecycleAction::Preview | AdminAuctionLifecycleAction::Open => {
            lifecycle_action_label(action)
        }
    }
}

pub(super) fn lifecycle_action_confirm_title(
    action: AdminAuctionLifecycleAction,
    auction_sequence: i64,
    world: &str,
) -> String {
    match action {
        AdminAuctionLifecycleAction::Preview => {
            t!("admin-auctions-confirm-preview", number: auction_sequence, world: world)
        }
        AdminAuctionLifecycleAction::Open => {
            t!("admin-auctions-confirm-open", number: auction_sequence, world: world)
        }
        AdminAuctionLifecycleAction::Cancel => {
            t!("admin-auctions-confirm-cancel", number: auction_sequence, world: world)
        }
        AdminAuctionLifecycleAction::FinalPhase => {
            t!("admin-auctions-confirm-final", number: auction_sequence, world: world)
        }
        AdminAuctionLifecycleAction::EarlySettlement => {
            t!("admin-auctions-confirm-early-settlement", number: auction_sequence, world: world)
        }
    }
}

pub(super) fn lifecycle_action_confirm_note(action: AdminAuctionLifecycleAction) -> String {
    match action {
        AdminAuctionLifecycleAction::Preview => t!("admin-auctions-confirm-preview-note"),
        AdminAuctionLifecycleAction::Open => t!("admin-auctions-confirm-open-note"),
        AdminAuctionLifecycleAction::Cancel => t!("admin-auctions-confirm-cancel-note"),
        AdminAuctionLifecycleAction::FinalPhase => t!("admin-auctions-confirm-final-note"),
        AdminAuctionLifecycleAction::EarlySettlement => {
            t!("admin-auctions-confirm-early-settlement-note")
        }
    }
}

// 镜像 Admin 生命周期 HTTP 入口的原因约束，长度按 UTF-8 字节计算。
const MAX_LIFECYCLE_REASON_BYTES: usize = 1_024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LifecycleReasonIssue {
    Required,
    TooLong,
    ControlCharacter,
}

impl LifecycleReasonIssue {
    pub(super) fn message(self) -> String {
        match self {
            Self::Required => t!("admin-field-required"),
            Self::TooLong => t!("admin-field-max-utf8-bytes", max: MAX_LIFECYCLE_REASON_BYTES),
            Self::ControlCharacter => t!("admin-field-no-control-characters"),
        }
    }
}

pub(super) fn validate_lifecycle_reason(reason: &str) -> Result<&str, LifecycleReasonIssue> {
    if reason.trim().is_empty() {
        return Err(LifecycleReasonIssue::Required);
    }
    if reason.len() > MAX_LIFECYCLE_REASON_BYTES {
        return Err(LifecycleReasonIssue::TooLong);
    }
    if reason.chars().any(char::is_control) {
        return Err(LifecycleReasonIssue::ControlCharacter);
    }
    Ok(reason.trim())
}

pub(crate) fn lifecycle_action_success_label(action: AdminAuctionLifecycleAction) -> String {
    match action {
        AdminAuctionLifecycleAction::Preview => t!("admin-auctions-action-preview-success"),
        AdminAuctionLifecycleAction::Open => t!("admin-auctions-action-open-success"),
        AdminAuctionLifecycleAction::Cancel => t!("admin-auctions-action-cancel-success"),
        AdminAuctionLifecycleAction::FinalPhase => t!("admin-auctions-action-final-success"),
        AdminAuctionLifecycleAction::EarlySettlement => {
            t!("admin-auctions-action-early-settlement-success")
        }
    }
}

pub(crate) fn auction_notice_label(notice: AdminAuctionNotice) -> String {
    match notice {
        AdminAuctionNotice::Lifecycle(action) => lifecycle_action_success_label(action),
        AdminAuctionNotice::Schedule => t!("admin-auctions-schedule-success"),
        AdminAuctionNotice::Created => t!("admin-auctions-create-success"),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::future::ready;
    use std::rc::Rc;

    use dioxus::prelude::{VNode, spawn, use_hook, use_signal};
    use gloo_net::http::Method;
    use uuid::Uuid;

    use super::{
        LifecycleReasonIssue, lifecycle_actions, pending_open_millis, validate_lifecycle_reason,
    };
    use crate::api::{
        AdminAuctionCapabilities, AdminAuctionLifecycleAction, AdminWriteFailure,
        execute_admin_auction_lifecycle,
    };
    use crate::surface::auctions::editor::observe_draft_write;
    use crate::surface::auctions::editor::tests::{WriteScript, mount_writer};

    const OPEN_AT: &str = "2026-11-14T12:00:00Z";

    fn open_millis() -> i64 {
        auction_time::Timestamp::parse_from_rfc3339(OPEN_AT).expect("open time").timestamp_millis()
    }

    fn capabilities(manual_early_open: bool) -> AdminAuctionCapabilities {
        AdminAuctionCapabilities {
            cancel_auction: true,
            manual_early_open,
            manual_early_settlement: true,
            manual_intake: false,
            manual_preview_open: true,
            modify_schedule: false,
        }
    }

    #[test]
    fn preview_open_action_appears_when_scheduled_time_arrives() {
        let open = open_millis();
        let before = lifecycle_actions("preview", &capabilities(false), OPEN_AT, open - 1);
        assert_eq!(before, vec![AdminAuctionLifecycleAction::Cancel]);
        assert_eq!(pending_open_millis("preview", false, OPEN_AT, open - 1), Some(open));
        let at = lifecycle_actions("preview", &capabilities(false), OPEN_AT, open);
        assert_eq!(
            at,
            vec![AdminAuctionLifecycleAction::Open, AdminAuctionLifecycleAction::Cancel]
        );
        assert_eq!(pending_open_millis("preview", false, OPEN_AT, open), None);
    }

    #[test]
    fn manual_early_open_does_not_wait_for_the_clock() {
        let early = open_millis() - 3_600_000;
        assert_eq!(
            lifecycle_actions("preview", &capabilities(true), OPEN_AT, early),
            vec![AdminAuctionLifecycleAction::Open, AdminAuctionLifecycleAction::Cancel]
        );
        assert_eq!(pending_open_millis("preview", true, OPEN_AT, early), None);
    }

    #[test]
    fn other_statuses_and_unparseable_times_never_need_the_clock() {
        let open = open_millis();
        for status in ["draft", "live", "final", "settling", "settled", "cancelled"] {
            assert_eq!(
                lifecycle_actions(status, &capabilities(false), OPEN_AT, open - 1),
                lifecycle_actions(status, &capabilities(false), OPEN_AT, open + 1),
                "{status} actions must not depend on time"
            );
            assert_eq!(pending_open_millis(status, false, OPEN_AT, open - 1), None);
        }
        assert_eq!(pending_open_millis("preview", false, "not a time", open), None);
        assert_eq!(
            lifecycle_actions("preview", &capabilities(false), "not a time", open),
            vec![AdminAuctionLifecycleAction::Cancel]
        );
    }

    #[test]
    fn lifecycle_reason_obeys_utf8_and_control_character_rules() {
        let exact_limit = "拍".repeat(341) + "a";
        assert_eq!(validate_lifecycle_reason(&exact_limit), Ok(exact_limit.as_str()));
        assert_eq!(validate_lifecycle_reason("  要核对  "), Ok("要核对"));
        assert_eq!(
            validate_lifecycle_reason(&(exact_limit + "b")),
            Err(LifecycleReasonIssue::TooLong)
        );
        assert_eq!(validate_lifecycle_reason(" "), Err(LifecycleReasonIssue::Required));
        for value in ["原因\n第二行", "原因\u{0085}"] {
            assert_eq!(
                validate_lifecycle_reason(value),
                Err(LifecycleReasonIssue::ControlCharacter)
            );
        }
    }

    #[test]
    fn only_verified_version_rejections_lock_the_auction_draft() {
        const AUCTION_ID: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
        let script = WriteScript::default();
        let feedbacks = Rc::new(RefCell::new(Vec::new()));
        let cases = [
            (Some(409), "stale_auction_state", "stale_auction_state", true),
            (Some(422), "stale_auction_state", "http_422", false),
            (None, "stale_auction_state", "stale_auction_state", false),
            (Some(409), "stale_state_version", "stale_state_version", false),
            (Some(409), "command_conflict", "command_conflict", false),
            (Some(409), "outcome_unknown", "outcome_unknown", false),
        ];
        for (status, code, _, _) in cases {
            if let Some(status) = status {
                script.reply_http(status, serde_json::json!({"code": code, "message": "拒绝", "data": null, "request_id": null}).to_string());
            }
        }
        let observed = feedbacks.clone();
        let writes = script.clone();
        let command_id = Uuid::from_u128(0x00000000_0000_4000_8000_000000000002);
        let (mut page, _) = mount_writer(move || {
            let feedback = use_signal(|| None);
            writes.install();
            let observed = observed.clone();
            use_hook(move || {
                spawn(async move {
                    for (status, code, _, _) in cases {
                        let result = if status.is_some() {
                            observe_draft_write(
                                execute_admin_auction_lifecycle(
                                    command_id,
                                    AUCTION_ID.to_owned(),
                                    AdminAuctionLifecycleAction::Cancel,
                                    7,
                                    Some("核对".to_owned()),
                                ),
                                feedback,
                                "stale_auction_state",
                            )
                            .await
                        } else {
                            observe_draft_write(
                                ready(Err(AdminWriteFailure::not_sent(code))),
                                feedback,
                                "stale_auction_state",
                            )
                            .await
                        };
                        assert!(result.is_err());
                        let outcome = feedback().expect("草稿失败反馈");
                        observed.borrow_mut().push((outcome.display_code, outcome.stale_write));
                    }
                });
            });
            VNode::empty()
        });
        page.rerender();
        assert_eq!(*feedbacks.borrow(), cases.map(|(_, _, code, stale)| (code.to_owned(), stale)));
        let captured = script.take_writes();
        assert_eq!(captured.len(), 5);
        for write in captured {
            assert_eq!(write.method, Method::POST);
            assert_eq!(write.url, format!("/api/v1/admin/auctions/{AUCTION_ID}/cancel"));
            assert_eq!(
                String::from_utf8(write.body).unwrap(),
                format!(
                    r#"{{"command_id":"{command_id}","expected_state_version":7,"reason":"核对"}}"#
                )
            );
        }
    }
}
