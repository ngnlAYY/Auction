use auction_application::delivery::{DELIVERY_ADMIN_RETIREMENT_REASONS, DeliveryAdminSnapshot};
use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use super::status::{
    reason_code_filtered, send_category, suppression_reason_for, template_from_payload,
    world_from_payload,
};
use super::validation::valid_snapshot;

#[test]
fn send_category_mirrors_sql_function_order() {
    assert_eq!(
        send_category("lot_watch", &json!({"operation": "send"})).as_deref(),
        Some("outbid")
    );
    assert_eq!(
        send_category("operations_alert", &json!({"operation": "send"})).as_deref(),
        Some("operations")
    );
    assert_eq!(
        send_category("seller_summary", &json!({"operation": "send"})).as_deref(),
        Some("settlement")
    );
    for template in ["register_key", "rekey_key", "rekey_result", "rebind_result"] {
        assert_eq!(
            send_category("mail_intake", &json!({"operation": "send", "template": template}))
                .as_deref(),
            Some("account"),
            "{template} must stay in the account class"
        );
    }
    assert_eq!(
        send_category(
            "mail_intake",
            &json!({"operation": "send", "template": "auction_submission_receipt"})
        )
        .as_deref(),
        Some("intake")
    );
    // 装备资产优先于结算来源归类，与 SQL CASE 的顺序一致。
    assert_eq!(
        send_category("settlement", &json!({"operation": "send", "attachment_ids": ["1"]}))
            .as_deref(),
        Some("equipment")
    );
    assert_eq!(
        send_category("settlement", &json!({"operation": "send"})).as_deref(),
        Some("settlement")
    );
    assert_eq!(send_category("manual", &json!({"operation": "send"})).as_deref(), Some("manual"));
    // Return/Take 不是 Send，不能占用发送类别。
    assert_eq!(send_category("lot_watch", &json!({"operation": "take"})), None);
}

#[test]
fn world_and_template_only_accept_known_shapes() {
    assert_eq!(world_from_payload(&json!({})).as_deref(), Some("main"));
    assert_eq!(world_from_payload(&json!({"world": "isekai"})).as_deref(), Some("isekai"));
    assert_eq!(world_from_payload(&json!({"world": "moon"})), None);
    assert_eq!(
        template_from_payload(&json!({"template": "lot_watch"})).as_deref(),
        Some("lot_watch")
    );
    assert_eq!(template_from_payload(&json!({"template": null})), None);
    assert_eq!(template_from_payload(&json!({"template": ""})), None);
}

#[test]
fn retirement_and_suppression_are_closed_sets() {
    assert_eq!(
        reason_code_filtered(Some("recipient_renamed"), &DELIVERY_ADMIN_RETIREMENT_REASONS)
            .as_deref(),
        Some("recipient_renamed")
    );
    assert_eq!(reason_code_filtered(Some("unknown"), &DELIVERY_ADMIN_RETIREMENT_REASONS), None);
    assert_eq!(
        suppression_reason_for(Some("invalid_payload"), None).as_deref(),
        Some("invalid_payload")
    );
    assert_eq!(
        suppression_reason_for(Some("retry_exhausted"), None).as_deref(),
        Some("retry_exhausted")
    );
    // 普通失败码不是抑制原因。
    assert_eq!(suppression_reason_for(Some("recipient_not_found"), None), None);
    assert_eq!(suppression_reason_for(None, Some("recipient_renamed")), None);
}

fn retired_player_notification(
    origin_kind: &str,
    reason_code: &str,
    failure_code: &str,
) -> DeliveryAdminSnapshot {
    let created_at = "2026-11-06T10:00:00Z".parse::<DateTime<Utc>>().expect("valid timestamp");
    DeliveryAdminSnapshot {
        // 退役只推进 fencing version；进门时 pending 的行保留 attempt_count 0 且没有 claimed 事实。
        id: Uuid::from_u128(1),
        origin_kind: origin_kind.to_owned(),
        origin_id: format!("{origin_kind}:1:alice"),
        recipient: "alice".to_owned(),
        cod: Some(0),
        status: "failed".to_owned(),
        template: Some("auction_seller_summary".to_owned()),
        category: Some("settlement".to_owned()),
        world: Some("main".to_owned()),
        retirement_reason: Some(reason_code.to_owned()),
        suppression_reason: None,
        lease_expires_at: None,
        fencing_version: 1,
        attempt_count: 0,
        claimed_at: None,
        sending_at: None,
        completed_at: Some(created_at + chrono::TimeDelta::seconds(5)),
        external_reference: None,
        failure_code: Some(failure_code.to_owned()),
        reason_code: Some(reason_code.to_owned()),
        take_required: false,
        returned_mail_id: None,
        created_at,
        updated_at: created_at + chrono::TimeDelta::seconds(5),
    }
}

#[test]
fn player_notification_preflight_retirements_validate() {
    // 收件人改名同时退役卖方汇总与换季提醒；换季提醒还可在边界已过或义务完成时退役。
    for (origin_kind, reason_code, failure_code) in [
        ("seller_summary", "recipient_renamed", "recipient_renamed"),
        ("season_reminder", "recipient_renamed", "recipient_renamed"),
        ("season_reminder", "season_boundary_passed", "season_notice_invalidated"),
        ("season_reminder", "season_reminder_resolved", "season_notice_invalidated"),
    ] {
        let snapshot = retired_player_notification(origin_kind, reason_code, failure_code);
        assert!(valid_snapshot(&snapshot), "{origin_kind}/{reason_code} 必须通过校验");
    }
    // 已领取后退役的行保留 claimed 事实与既有 attempt 计数。
    let mut claimed = retired_player_notification(
        "season_reminder",
        "season_boundary_passed",
        "season_notice_invalidated",
    );
    claimed.claimed_at = Some(claimed.created_at + chrono::TimeDelta::seconds(1));
    claimed.attempt_count = 1;
    claimed.fencing_version = 2;
    assert!(valid_snapshot(&claimed));
}

#[test]
fn player_notification_retirements_still_reject_malformed_rows() {
    // 换季原因只属于换季提醒；卖方汇总只能因改名退役。
    let mut wrong_origin = retired_player_notification(
        "seller_summary",
        "season_boundary_passed",
        "season_notice_invalidated",
    );
    assert!(!valid_snapshot(&wrong_origin));
    wrong_origin.origin_kind = "lot_watch".to_owned();
    assert!(!valid_snapshot(&wrong_origin));
    // 失败码必须与退役语义一致。
    let wrong_failure = retired_player_notification(
        "season_reminder",
        "season_reminder_resolved",
        "recipient_renamed",
    );
    assert!(!valid_snapshot(&wrong_failure));
    // 一旦出现外部写入事实就不能再套用预检退役时间链。
    let mut sent = retired_player_notification(
        "season_reminder",
        "season_boundary_passed",
        "season_notice_invalidated",
    );
    sent.sending_at = Some(sent.created_at + chrono::TimeDelta::seconds(2));
    assert!(!valid_snapshot(&sent));
    let mut referenced =
        retired_player_notification("seller_summary", "recipient_renamed", "recipient_renamed");
    referenced.external_reference = Some("3120869".to_owned());
    assert!(!valid_snapshot(&referenced));
    // completed 事实仍是必需项，且必须落在 created/updated 窗口内。
    let mut missing_completed = retired_player_notification(
        "season_reminder",
        "season_reminder_resolved",
        "season_notice_invalidated",
    );
    missing_completed.completed_at = None;
    assert!(!valid_snapshot(&missing_completed));
    let mut late = retired_player_notification(
        "season_reminder",
        "season_boundary_passed",
        "season_notice_invalidated",
    );
    late.updated_at = late.created_at;
    assert!(!valid_snapshot(&late));
    // 未推进 fencing version 的行不能证明退役已经提交。
    let mut unfenced =
        retired_player_notification("seller_summary", "recipient_renamed", "recipient_renamed");
    unfenced.fencing_version = 0;
    assert!(!valid_snapshot(&unfenced));
}

#[test]
fn recovery_quarantine_from_pending_delivery_validates() {
    let mut snapshot =
        retired_player_notification("settlement", "recipient_renamed", "recipient_renamed");
    snapshot.status = "manual_reconcile".to_owned();
    snapshot.reason_code = None;
    snapshot.retirement_reason = None;
    snapshot.failure_code = Some("external_recovery_sent_match".to_owned());
    snapshot.claimed_at = None;
    snapshot.attempt_count = 0;
    // 从未 claim 的 pending 行被恢复核对直接隔离：没有 sending 事实，但 completed 已写入。
    assert!(valid_snapshot(&snapshot));
    snapshot.failure_code = Some("external_recovery_payload_match".to_owned());
    assert!(valid_snapshot(&snapshot));
    snapshot.failure_code = Some("external_recovery_unresolved_send".to_owned());
    assert!(valid_snapshot(&snapshot));

    // 外部写入事实、未知失败码或缺失 completed 仍必须拒绝。
    let mut sent = snapshot.clone();
    sent.sending_at = Some(sent.created_at + chrono::TimeDelta::seconds(2));
    assert!(!valid_snapshot(&sent));
    let mut unknown = snapshot.clone();
    unknown.failure_code = Some("other_failure".to_owned());
    assert!(!valid_snapshot(&unknown));
    let mut missing_completed = snapshot;
    missing_completed.completed_at = None;
    assert!(!valid_snapshot(&missing_completed));
}
