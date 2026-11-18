//! Admin 投递 decoder 与详情校验的回归用例。

use auction_contracts::{
    AdminDeliveryAttemptResponse, AdminDeliveryCustodyResponse, AdminDeliveryDetailResponse,
    AdminDeliveryDispositionResponse, AdminDeliveryEvidenceResponse, AdminDeliveryResponse,
    AdminDeliverySentProofPreviewResponse, AdminDeliverySentProofResponse,
};
use auction_time::Timestamp;
use serde_json::{Value, json};
use uuid::Uuid;

use super::player_support::player_support_url;
use super::queue::{DeliverySearchFilters, delivery_list_url};
use super::{
    decode::{StrictDeliveryEnvelope, decode_delivery_list},
    decode_confirmation, decode_detail, decode_preview, delivery_detail_url,
};

fn time(offset: u32) -> String {
    Timestamp::from_ymd_hms(2026, 11, 2, 1, offset, 0).expect("timestamp").to_rfc3339()
}

// `valid_uuid` 会校验版本位与变体位，测试夹具必须是规范 UUID。
const DELIVERY_ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
const OTHER_ID: &str = "2c9e63a1-7f4b-4a0e-b3d5-9f8c1e2d4b60";

fn delivery() -> AdminDeliveryResponse {
    AdminDeliveryResponse {
        id: DELIVERY_ID.to_owned(),
        origin_kind: "settlement".to_owned(),
        origin_id: "42".to_owned(),
        recipient: "Sample".to_owned(),
        cod: Some(0),
        status: "manual_reconcile".to_owned(),
        template: Some("settlement_seller_payout".to_owned()),
        category: Some("settlement".to_owned()),
        world: Some("main".to_owned()),
        retirement_reason: None,
        suppression_reason: None,
        lease_expires_at: None,
        fencing_version: 3,
        attempt_count: 1,
        claimed_at: Some(time(0)),
        sending_at: Some(time(0)),
        completed_at: Some(time(1)),
        external_reference: None,
        failure_code: Some("write_unknown".to_owned()),
        reason_code: None,
        take_required: false,
        returned_mail_id: None,
        created_at: time(0),
        updated_at: time(1),
    }
}

fn attempt() -> AdminDeliveryAttemptResponse {
    AdminDeliveryAttemptResponse {
        operation: "send".to_owned(),
        fencing_version: 3,
        world_id: "main".to_owned(),
        sending_at: time(0),
        recipient: Some("Sample".to_owned()),
        subject: Some("Auction settlement".to_owned()),
        body_digest: Some("ab".repeat(32)),
        credits: Some(300_000),
        cod: Some(0),
        equipment_ids: Vec::new(),
        external_mail_id: None,
        credential: None,
        bound_sent_mail_id: None,
    }
}

fn detail() -> AdminDeliveryDetailResponse {
    AdminDeliveryDetailResponse {
        delivery: delivery(),
        attempts: vec![attempt()],
        evidence: vec![AdminDeliveryEvidenceResponse {
            reference: "mailbox_sweep:7000001".to_owned(),
            kind: "mailbox_sweep".to_owned(),
            external_mail_id: Some("7000001".to_owned()),
            observed_at: Some(time(2)),
            sent_at: None,
            recipient: Some("Sample".to_owned()),
            subject: Some("Auction settlement".to_owned()),
            body_digest: Some("cd".repeat(32)),
            collected: Some(false),
            cod_pending: Some(0),
            remaining_credits: Some(300_000),
            remaining_equipment_count: Some(0),
            receipt: None,
        }],
        custody: AdminDeliveryCustodyResponse {
            collected: Some(false),
            cod_pending: Some(0),
            remaining_credits: Some(300_000),
            receipt: None,
            observed_at: Some(time(2)),
            operator_confirmed_collected: false,
        },
        dispositions: Vec::new(),
        obligation: None,
        recovery: None,
    }
}

#[test]
fn accepts_complete_detail() {
    let decoded = decode_detail(StrictDeliveryEnvelope {
        code: "ok".to_owned(),
        message: "loaded".to_owned(),
        data: detail(),
    })
    .expect("decode");
    assert_eq!(decoded.attempts.len(), 1);
}

/// 对应 storage 的 `retired_player_notification` 夹具与投稿回执退役投影。
fn retired_before_send(origin_kind: &str, reason_code: &str, failure_code: &str) -> Value {
    json!({
        "id": DELIVERY_ID,
        "origin_kind": origin_kind,
        "origin_id": format!("{origin_kind}:1:alice"),
        "recipient": "alice",
        "cod": 0,
        "status": "failed",
        "world": "main",
        "retirement_reason": reason_code,
        "lease_expires_at": null,
        "fencing_version": 1,
        "attempt_count": 0,
        "claimed_at": null,
        "sending_at": null,
        "completed_at": "2026-11-06T10:00:05.000000Z",
        "external_reference": null,
        "failure_code": failure_code,
        "reason_code": reason_code,
        "created_at": "2026-11-06T10:00:00.000000Z",
        "updated_at": "2026-11-06T10:00:05.000000Z"
    })
}

fn check_before_send_projection(delivery: Value, valid: bool) {
    let list = json!({
        "code": "ok",
        "message": "loaded",
        "data": {
            "deliveries": [delivery],
            "pagination": {"page": 1, "page_size": 10, "total": 1}
        }
    });
    let list = serde_json::from_value(list).unwrap();
    let list_valid = decode_delivery_list(list, 1, 10).is_ok();

    let mut data = detail();
    data.attempts.clear();
    data.delivery = serde_json::from_value(delivery.clone()).unwrap();
    let response =
        StrictDeliveryEnvelope { code: "ok".to_owned(), message: "loaded".to_owned(), data };
    let detail_valid = decode_detail(response).is_ok();
    assert_eq!((list_valid, detail_valid), (valid, valid), "列表与详情投影 {delivery}");
}

#[test]
fn delivery_accepts_server_preflight_retirements() {
    for (origin, reason, failure) in [
        ("lot_watch", "lot_watch_invalidated", "lot_watch_invalidated"),
        ("lot_watch", "recipient_renamed", "recipient_renamed"),
        ("seller_summary", "recipient_renamed", "recipient_renamed"),
        ("season_reminder", "recipient_renamed", "recipient_renamed"),
        ("season_reminder", "season_boundary_passed", "season_notice_invalidated"),
        ("season_reminder", "season_reminder_resolved", "season_notice_invalidated"),
        ("mail_intake", "auction_opened", "auction_notice_invalidated"),
    ] {
        check_before_send_projection(retired_before_send(origin, reason, failure), true);
    }

    let mut claimed = retired_before_send(
        "season_reminder",
        "season_boundary_passed",
        "season_notice_invalidated",
    );
    claimed["claimed_at"] = json!("2026-11-06T10:00:01.000000Z");
    claimed["attempt_count"] = json!(1);
    claimed["fencing_version"] = json!(2);
    check_before_send_projection(claimed, true);
}

#[test]
fn delivery_rejects_malformed_preflight_retirements() {
    let valid = retired_before_send(
        "season_reminder",
        "season_boundary_passed",
        "season_notice_invalidated",
    );
    for (key, invalid) in [
        ("origin_kind", json!("seller_summary")),
        ("failure_code", json!("recipient_renamed")),
        ("completed_at", Value::Null),
        ("completed_at", json!("2026-11-06T10:00:06.000000Z")),
        ("claimed_at", json!("2026-11-06T10:00:01.000000Z")),
        ("external_reference", json!("3120869")),
        ("fencing_version", json!(0)),
        ("cod", json!(1)),
        ("retirement_reason", json!("season_reminder_resolved")),
        ("suppression_reason", json!("retry_exhausted")),
    ] {
        let mut malformed = valid.clone();
        malformed[key] = invalid;
        check_before_send_projection(malformed, false);
    }
    // 即使正常失败的完整时间链有效，换季退役也不能携带发送事实。
    let mut sent = valid;
    sent["claimed_at"] = json!("2026-11-06T10:00:01.000000Z");
    sent["sending_at"] = json!("2026-11-06T10:00:02.000000Z");
    sent["attempt_count"] = json!(1);
    let mut missing_reason = sent.clone();
    missing_reason.as_object_mut().unwrap().remove("reason_code");
    check_before_send_projection(missing_reason, false);
    sent.as_object_mut().unwrap().remove("retirement_reason");
    check_before_send_projection(sent, false);
}

fn preparation_failure(failure_code: &str) -> Value {
    let mut failed = serde_json::to_value(delivery()).unwrap();
    failed["status"] = json!("failed");
    failed["sending_at"] = Value::Null;
    failed["failure_code"] = json!(failure_code);
    failed["suppression_reason"] = json!(failure_code);
    failed
}

#[test]
fn delivery_rejects_suppression_outside_its_projected_reason_and_state() {
    let prepared = preparation_failure("invalid_payload");
    check_before_send_projection(prepared.clone(), true);
    for label in ["unknown_reason", "retry_exhausted"] {
        let mut mislabeled = prepared.clone();
        mislabeled["suppression_reason"] = json!(label);
        check_before_send_projection(mislabeled, false);
    }

    let mut sent = serde_json::to_value(delivery()).unwrap();
    sent["status"] = json!("failed");
    sent["failure_code"] = json!("retry_exhausted");
    check_before_send_projection(sent.clone(), true);
    sent["suppression_reason"] = json!("retry_exhausted");
    check_before_send_projection(sent, false);

    let mut pending = prepared;
    pending["status"] = json!("pending");
    pending["failure_code"] = Value::Null;
    pending["claimed_at"] = Value::Null;
    pending["completed_at"] = Value::Null;
    pending["reason_code"] = json!("invalid_payload");
    check_before_send_projection(pending, false);
}

#[test]
fn delivery_suppression_prefers_failure_code_and_can_use_reason_code() {
    let mut alert = preparation_failure("retry_exhausted");
    alert["origin_kind"] = json!("operations_alert");
    alert["claimed_at"] = Value::Null;
    alert["attempt_count"] = json!(0);
    alert["fencing_version"] = json!(0);
    alert["reason_code"] = json!("operations_alert_batch_terminal");
    check_before_send_projection(alert.clone(), true);
    alert["suppression_reason"] = json!("operations_alert_batch_terminal");
    check_before_send_projection(alert, false);

    let mut conflict = preparation_failure("verified_not_sent");
    conflict["origin_kind"] = json!("mail_intake");
    conflict["reason_code"] = json!("mail_disposition_conflict");
    conflict["suppression_reason"] = json!("mail_disposition_conflict");
    check_before_send_projection(conflict, true);
}

#[test]
fn delivery_rejects_preparation_failure_disguised_as_disposition_conflict() {
    let mut failed = preparation_failure("task_panic");
    failed["origin_kind"] = json!("mail_intake");
    failed["reason_code"] = json!("mail_disposition_conflict");
    check_before_send_projection(failed.clone(), false);
    failed.as_object_mut().unwrap().remove("reason_code");
    check_before_send_projection(failed, true);
}

#[test]
fn delivery_checks_disposition_conflict_before_recovery_superseded() {
    let mut conflict = preparation_failure("recovery_superseded");
    conflict["origin_kind"] = json!("mail_intake");
    conflict["category"] = json!("intake");
    conflict["template"] = json!("auction_equipment_rejected");
    conflict["reason_code"] = json!("mail_disposition_conflict");
    conflict["suppression_reason"] = json!("mail_disposition_conflict");
    check_before_send_projection(conflict.clone(), true);

    // 处置冲突已有领取时间时必须有尝试记录。
    let mut unattempted = conflict.clone();
    unattempted["attempt_count"] = json!(0);
    check_before_send_projection(unattempted, false);

    conflict["sending_at"] = json!(time(0));
    conflict.as_object_mut().unwrap().remove("suppression_reason");
    check_before_send_projection(conflict, false);
}

#[test]
fn delivery_rejects_attempt_count_outside_the_storage_range() {
    let mut projected =
        retired_before_send("seller_summary", "recipient_renamed", "recipient_renamed");
    projected["attempt_count"] = json!(i32::MAX);
    check_before_send_projection(projected.clone(), true);
    projected["attempt_count"] = json!(i32::MAX as u32 + 1);
    check_before_send_projection(projected, false);
}

#[test]
fn delivery_requires_world_for_returned_mail_reference() {
    let mut returned = serde_json::to_value(delivery()).unwrap();
    returned["failure_code"] = json!("recipient_not_found");
    returned["reason_code"] = json!("recipient_not_found");
    returned["take_required"] = json!(true);
    returned["returned_mail_id"] = json!("3120869");
    returned["external_reference"] = json!("3120869");
    check_before_send_projection(returned.clone(), true);
    returned.as_object_mut().unwrap().remove("world");
    check_before_send_projection(returned, false);
}

#[test]
fn delivery_accepts_recovery_quarantine_before_send() {
    let mut quarantined =
        retired_before_send("settlement", "recipient_renamed", "recipient_renamed");
    quarantined["status"] = json!("manual_reconcile");
    let object = quarantined.as_object_mut().unwrap();
    object.remove("reason_code");
    object.remove("retirement_reason");
    for failure in [
        "external_recovery_sent_match",
        "external_recovery_payload_match",
        "external_recovery_unresolved_send",
    ] {
        quarantined["failure_code"] = json!(failure);
        check_before_send_projection(quarantined.clone(), true);
    }
    let mut unknown = quarantined.clone();
    unknown["failure_code"] = json!("other_failure");
    check_before_send_projection(unknown, false);
    quarantined["completed_at"] = Value::Null;
    check_before_send_projection(quarantined, false);
}

#[test]
fn delivery_rejects_recovery_quarantine_with_send_facts() {
    let sent = serde_json::to_value(delivery()).unwrap();
    check_before_send_projection(sent.clone(), true);
    for failure in [
        "external_recovery_sent_match",
        "external_recovery_payload_match",
        "external_recovery_unresolved_send",
    ] {
        let mut quarantined = sent.clone();
        quarantined["failure_code"] = json!(failure);
        check_before_send_projection(quarantined, false);
    }
}

/// 被恢复终结的原结算投递（`failed/recovery_superseded`）没有发送时间，只在扩展投影里出现：
/// 解码接受这一确定未发送的终态，来源不是结算或带发送时间时拒绝。
#[test]
fn accepts_recovery_superseded_settlement_delivery_only_without_send_facts() {
    let decode = |delivery: AdminDeliveryResponse| {
        let mut data = detail();
        data.delivery = delivery;
        data.attempts.clear();
        decode_detail(StrictDeliveryEnvelope {
            code: "ok".to_owned(),
            message: "loaded".to_owned(),
            data,
        })
    };
    let mut superseded = delivery();
    superseded.status = "failed".to_owned();
    superseded.failure_code = Some("recovery_superseded".to_owned());
    superseded.claimed_at = None;
    superseded.sending_at = None;
    superseded.attempt_count = 0;
    assert!(decode(superseded.clone()).is_ok());
    let mut claimed = superseded.clone();
    claimed.claimed_at = Some(time(0));
    assert!(decode(claimed).is_ok());
    let mut sent = superseded.clone();
    sent.claimed_at = Some(time(0));
    sent.sending_at = Some(time(0));
    sent.attempt_count = 1;
    assert!(decode(sent).is_err());
    let mut other_origin = superseded;
    other_origin.origin_kind = "lot_watch".to_owned();
    assert!(decode(other_origin).is_err());
}

#[test]
fn accepts_operator_acknowledged_disposition_in_detail() {
    let mut data = detail();
    data.dispositions.push(AdminDeliveryDispositionResponse {
        command_id: OTHER_ID.to_owned(),
        kind: "operator_acknowledged".to_owned(),
        occurred_at: time(2),
        status: None,
        fencing_version: Some(3),
        failure_code: None,
        external_reference: None,
        actor: Some("admin".to_owned()),
        reason: Some("handled outside the outbox".to_owned()),
    });
    let decoded = decode_detail(StrictDeliveryEnvelope {
        code: "ok".to_owned(),
        message: "loaded".to_owned(),
        data,
    })
    .expect("operator_acknowledged must remain decodable");
    assert_eq!(decoded.dispositions[0].kind, "operator_acknowledged");
}

#[test]
fn accepts_unified_disposition_kinds_in_detail() {
    let mut data = detail();
    data.dispositions.push(AdminDeliveryDispositionResponse {
        command_id: OTHER_ID.to_owned(),
        kind: "auto_sent_proof".to_owned(),
        occurred_at: time(2),
        status: None,
        fencing_version: Some(3),
        failure_code: Some("sent_recovered_frozen_v1".to_owned()),
        external_reference: None,
        actor: Some("delivery-worker".to_owned()),
        reason: None,
    });
    assert!(
        decode_detail(StrictDeliveryEnvelope {
            code: "ok".to_owned(),
            message: "loaded".to_owned(),
            data,
        })
        .is_ok()
    );
}

#[test]
fn rejects_send_attempt_without_frozen_fields() {
    let mut data = detail();
    data.attempts[0].subject = None;
    assert!(
        decode_detail(StrictDeliveryEnvelope {
            code: "ok".to_owned(),
            message: "loaded".to_owned(),
            data,
        })
        .is_err()
    );
}

#[test]
fn rejects_unknown_evidence_kind_and_uppercase_digest() {
    let mut data = detail();
    data.evidence[0].kind = "draft_discard".to_owned();
    assert!(
        decode_detail(StrictDeliveryEnvelope {
            code: "ok".to_owned(),
            message: "loaded".to_owned(),
            data,
        })
        .is_err()
    );

    let mut data = detail();
    data.evidence[0].body_digest = Some("AB".repeat(32));
    assert!(
        decode_detail(StrictDeliveryEnvelope {
            code: "ok".to_owned(),
            message: "loaded".to_owned(),
            data,
        })
        .is_err()
    );
}

#[test]
fn rejects_take_attempt_with_send_fields_only() {
    let mut data = detail();
    data.attempts[0] = AdminDeliveryAttemptResponse {
        operation: "take".to_owned(),
        external_mail_id: None,
        ..attempt()
    };
    assert!(
        decode_detail(StrictDeliveryEnvelope {
            code: "ok".to_owned(),
            message: "loaded".to_owned(),
            data,
        })
        .is_err()
    );
}

#[test]
fn rejects_preview_for_other_delivery_or_eligible_with_blockers() {
    let preview = |delivery_id: &str, eligible: bool, blockers: Vec<&str>| {
        AdminDeliverySentProofPreviewResponse {
            eligible,
            blockers: blockers.into_iter().map(str::to_owned).collect(),
            delivery_id: delivery_id.to_owned(),
            expected_fencing_version: 3,
            external_reference: "mooglemail:main:send:7000001".to_owned(),
            requires_collected_asset_acceptance: false,
            requires_attempt_history_acceptance: false,
            corrects_admin_failure: true,
        }
    };
    let envelope = |data| StrictDeliveryEnvelope {
        code: "ok".to_owned(),
        message: "preview".to_owned(),
        data,
    };
    assert!(decode_preview(envelope(preview(DELIVERY_ID, true, Vec::new())), DELIVERY_ID).is_ok());
    assert!(decode_preview(envelope(preview(OTHER_ID, true, Vec::new())), DELIVERY_ID).is_err());
    assert!(
        decode_preview(
            envelope(preview(DELIVERY_ID, true, vec!["sender_player_name_mismatch"])),
            DELIVERY_ID
        )
        .is_err()
    );
}

#[test]
fn confirmation_must_echo_command_and_delivery() {
    let command_id = Uuid::from_bytes([9; 16]);
    let data = AdminDeliverySentProofResponse {
        command_id: command_id.to_string(),
        delivery_id: DELIVERY_ID.to_owned(),
        external_reference: "mooglemail:main:send:7000001".to_owned(),
        replayed: false,
    };
    let envelope = || StrictDeliveryEnvelope {
        code: "ok".to_owned(),
        message: "confirmed".to_owned(),
        data: data.clone(),
    };
    assert!(decode_confirmation(envelope(), command_id, DELIVERY_ID).is_ok());
    assert!(decode_confirmation(envelope(), Uuid::from_bytes([8; 16]), DELIVERY_ID).is_err());
    assert!(decode_confirmation(envelope(), command_id, OTHER_ID).is_err());
}

/// 本页面认识结算退件 Take 来源与装备收回处置，详情必须显式请求扩展投影；缺省响应留给升级前的
/// 页面。
#[test]
fn detail_url_requests_the_settlement_returns_projection() {
    assert_eq!(
        delivery_detail_url(DELIVERY_ID),
        format!("/api/v1/admin/deliveries/{DELIVERY_ID}?include=settlement_returns")
    );
}

/// 列表同样总是请求扩展投影（缺省列表不含结算退件 Take），并且只提交非空的筛选。
#[test]
fn list_url_requests_the_settlement_returns_projection() {
    assert_eq!(
        delivery_list_url("", &DeliverySearchFilters::default(), 1, 50),
        "/api/v1/admin/deliveries?page=1&page_size=50&include=settlement_returns"
    );
    let filters = DeliverySearchFilters {
        world: "isekai".to_owned(),
        reference: DELIVERY_ID.to_owned(),
        ..DeliverySearchFilters::default()
    };
    assert_eq!(
        delivery_list_url("manual_reconcile", &filters, 0, 20),
        format!(
            "/api/v1/admin/deliveries?page=1&page_size=20&status=manual_reconcile\
             &reference={DELIVERY_ID}&world=isekai&include=settlement_returns"
        )
    );
}

/// 玩家支持同样总是请求扩展投影（缺省投递分区不含结算退件 Take），检索值照旧经表单编码。
#[test]
fn player_support_url_requests_the_settlement_returns_projection() {
    assert_eq!(
        player_support_url("user", "Sample Name"),
        "/api/v1/admin/player-support?user=Sample+Name&include=settlement_returns"
    );
    assert_eq!(
        player_support_url("reference", DELIVERY_ID),
        format!("/api/v1/admin/player-support?reference={DELIVERY_ID}&include=settlement_returns")
    );
}
