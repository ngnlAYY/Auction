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
) -> String {
    match action {
        AdminAuctionLifecycleAction::Preview => {
            t!("admin-auctions-confirm-preview", number: auction_sequence)
        }
        AdminAuctionLifecycleAction::Open => {
            t!("admin-auctions-confirm-open", number: auction_sequence)
        }
        AdminAuctionLifecycleAction::Cancel => {
            t!("admin-auctions-confirm-cancel", number: auction_sequence)
        }
        AdminAuctionLifecycleAction::FinalPhase => {
            t!("admin-auctions-confirm-final", number: auction_sequence)
        }
        AdminAuctionLifecycleAction::EarlySettlement => {
            t!("admin-auctions-confirm-early-settlement", number: auction_sequence)
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
    use super::{lifecycle_actions, pending_open_millis};
    use crate::api::{AdminAuctionCapabilities, AdminAuctionLifecycleAction};

    const OPEN_AT: &str = "2026-11-14T12:00:00Z";

    fn open_millis() -> i64 {
        chrono::DateTime::parse_from_rfc3339(OPEN_AT).expect("open time").timestamp_millis()
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
}
