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
use super::responses::{decode_detail, decode_detail_with_actions};
use super::{
    decode::{StrictDeliveryEnvelope, decode_delivery_list},
    decode_confirmation, decode_preview, delivery_detail_url,
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
            remaining_equipment_ids: None,
            pre_collection_equipment_ids: None,
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

#[test]
fn delivery_decodes_actual_http_failure_and_action_projection() {
    use auction_contracts::AdminDeliveryActionBlocker;

    // HTTP 的映射测试对照同一个序列化样本，保护详情/列表共用的安全投影与准入闭集。
    let fixture: Value =
        serde_json::from_str(include_str!("blocked_registration_reply.json")).unwrap();
    let decoded = decode_detail_with_actions(
        serde_json::from_value(fixture.clone()).unwrap(),
        DELIVERY_ID,
        false,
    )
    .unwrap();
    assert_eq!(
        decoded.actions.retry_failed.blocker(),
        Some(AdminDeliveryActionBlocker::RecipientRenamed)
    );
    assert_eq!(decoded.actions.acknowledge.blocker(), None);
    check_before_send_projection(fixture["data"]["detail"]["delivery"].clone(), true);
    assert!(
        decode_detail_with_actions(
            serde_json::from_value(fixture.clone()).unwrap(),
            OTHER_ID,
            false
        )
        .is_err()
    );

    for (field, invalid) in [
        ("retry_failed", json!({"state": "blocked", "reason": "future_reason"})),
        ("release", json!({"state": "blocked"})),
        ("acknowledge", json!({"state": "eligible", "reason": "already_disposed"})),
        ("reconcile_retry", Value::Null),
    ] {
        let mut malformed = fixture.clone();
        malformed["data"]["actions"][field] = invalid;
        let result = serde_json::from_value(malformed)
            .map(|payload| decode_detail_with_actions(payload, DELIVERY_ID, false));
        assert!(result.is_err(), "必须拒绝损坏的动作投影 {field}");
    }
}

#[test]
fn delivery_decodes_new_blockers_serialized_by_http() {
    use auction_contracts::AdminDeliveryActionBlocker;

    let mut fixture: Value =
        serde_json::from_str(include_str!("blocked_registration_reply.json")).unwrap();
    let blockers: Vec<Value> =
        serde_json::from_str(include_str!("disposition_action_blockers.json")).unwrap();
    fixture["data"]["actions"]["retry_failed"] = blockers[0].clone();
    fixture["data"]["actions"]["reconcile_failed"] = blockers[1].clone();
    fixture["data"]["actions"]["release"] = blockers[2].clone();
    let decoded =
        decode_detail_with_actions(serde_json::from_value(fixture).unwrap(), DELIVERY_ID, false)
            .unwrap();
    assert_eq!(
        decoded.actions.retry_failed.blocker(),
        Some(AdminDeliveryActionBlocker::SettlementFulfillmentState)
    );
    assert_eq!(
        decoded.actions.reconcile_failed.blocker(),
        Some(AdminDeliveryActionBlocker::MissingSendingFence)
    );
    assert_eq!(
        decoded.actions.release.blocker(),
        Some(AdminDeliveryActionBlocker::DeliveryCommandSnapshotUnreadable)
    );
    assert_eq!(decoded.actions.acknowledge.blocker(), None);
}

/// 摘录 v0.1.8 的 api/delivery/decode.rs 与 api/validation.rs 中适用于这两种回信失败形状的条件。
fn v018_accepts_mail_intake_failure(row: &AdminDeliveryResponse) -> bool {
    use crate::api::valid_optional_text;

    const V018_JSON_MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
    let parse = |time: &str| Timestamp::parse_from_rfc3339(time).ok();
    let is_zero =
        |time: Timestamp| time.timestamp() == -62_135_596_800 && time.timestamp_subsec_nanos() == 0;
    let (Some(created), Some(updated), Some(claimed), Some(completed)) = (
        parse(&row.created_at),
        parse(&row.updated_at),
        row.claimed_at.as_deref().and_then(parse),
        row.completed_at.as_deref().and_then(parse),
    ) else {
        return false;
    };
    if !crate::api::valid_uuid(&row.id)
        || row.origin_kind != "mail_intake"
        || row.status != "failed"
        || !crate::api::valid_trimmed_text(&row.origin_id)
        || !crate::api::valid_trimmed_text(&row.recipient)
        || !valid_optional_text(row.template.as_deref())
        || row.template.as_deref().is_some_and(|template| template.len() > 64)
        || row.category.as_deref().is_some_and(|category| {
            !matches!(
                category,
                "outbid"
                    | "account"
                    | "intake"
                    | "settlement"
                    | "equipment"
                    | "operations"
                    | "manual"
            )
        })
        || row.world.as_deref().is_some_and(|world| !matches!(world, "main" | "isekai"))
        || row.cod.is_some_and(|cod| !(0..=V018_JSON_MAX_SAFE_INTEGER).contains(&cod))
        || !valid_optional_text(row.retirement_reason.as_deref())
        || !valid_optional_text(row.suppression_reason.as_deref())
        || !valid_optional_text(row.reason_code.as_deref())
        || row.returned_mail_id.as_deref().is_some_and(|mid| {
            !(1..=32).contains(&mid.len()) || !mid.bytes().all(|byte| byte.is_ascii_digit())
        })
        || row.lease_expires_at.is_some()
        || row.external_reference.is_some()
        || !(1..=V018_JSON_MAX_SAFE_INTEGER).contains(&row.fencing_version)
        || row.attempt_count == 0
        || is_zero(created)
        || is_zero(updated)
        || updated < created
        || claimed < created
        || matches!(
            row.reason_code.as_deref(),
            Some("lot_watch_invalidated" | "recipient_renamed" | "mail_disposition_conflict")
        )
        || row.failure_code.as_deref() == Some("recovery_superseded")
    {
        return false;
    }
    if row.sending_at.is_none() && row.failure_code.as_deref() == Some("invalid_payload") {
        row.reason_code.is_none()
            && !is_zero(claimed)
            && !is_zero(completed)
            && completed >= claimed
            && completed <= updated
            && !row.take_required
            && row.returned_mail_id.is_none()
    } else {
        row.failure_code.as_deref().is_some_and(crate::api::valid_trimmed_text)
            && row
                .sending_at
                .as_deref()
                .and_then(parse)
                .is_some_and(|sending| sending >= claimed && completed >= sending)
    }
}

#[test]
fn login_key_reply_failure_shapes_keep_v018_list_compatibility() {
    let read_http_projection = |fixture| -> AdminDeliveryResponse {
        let fixture: Value = serde_json::from_str(fixture).unwrap();
        let delivery = fixture["data"]["detail"]["delivery"].clone();
        check_before_send_projection(delivery.clone(), true);
        decode_detail_with_actions(serde_json::from_value(fixture).unwrap(), DELIVERY_ID, false)
            .unwrap();
        let row = serde_json::from_value(delivery).unwrap();
        assert!(v018_accepts_mail_intake_failure(&row));
        row
    };
    let mut after_fence = read_http_projection(include_str!("blocked_registration_reply.json"));
    let mut before_fence =
        read_http_projection(include_str!("blocked_registration_reply_before_fence.json"));

    after_fence.sending_at = None;
    assert!(!v018_accepts_mail_intake_failure(&after_fence));
    before_fence.reason_code = Some("recipient_renamed".to_owned());
    before_fence.retirement_reason = before_fence.reason_code.clone();
    before_fence.failure_code = before_fence.reason_code.clone();
    before_fence.suppression_reason = None;
    assert!(!v018_accepts_mail_intake_failure(&before_fence));
    check_before_send_projection(serde_json::to_value(&before_fence).unwrap(), false);
}

#[test]
fn v018_login_key_reply_projection_rejects_metadata_outside_release_boundaries() {
    let fixture: Value =
        serde_json::from_str(include_str!("blocked_registration_reply_before_fence.json")).unwrap();
    let mut boundary: AdminDeliveryResponse =
        serde_json::from_value(fixture["data"]["detail"]["delivery"].clone()).unwrap();
    boundary.template = Some("a".repeat(64));
    boundary.cod = Some(9_007_199_254_740_991);
    assert!(v018_accepts_mail_intake_failure(&boundary));

    let delivery = &fixture["data"]["detail"]["delivery"];
    for (field, invalid) in [
        ("template", json!("界".repeat(22))),
        ("category", json!("future_category")),
        ("world", json!("future_world")),
        ("cod", json!(-1)),
        ("cod", json!(9_007_199_254_740_992_i64)),
    ] {
        let mut row = delivery.clone();
        row[field] = invalid;
        let row = serde_json::from_value(row).unwrap();
        assert!(!v018_accepts_mail_intake_failure(&row), "v0.1.8 应拒绝超界的 {field}");
    }
}

#[test]
fn v018_login_key_reply_projection_rejects_zero_times_at_the_release_boundaries() {
    let fixture: Value =
        serde_json::from_str(include_str!("blocked_registration_reply_before_fence.json")).unwrap();
    let delivery = &fixture["data"]["detail"]["delivery"];
    let before_zero = json!("0000-12-31T23:59:59Z");
    let zero = json!("0001-01-01T00:00:00Z");
    for (field, earlier_fields) in [
        ("created_at", &[][..]),
        ("updated_at", &["created_at", "claimed_at", "completed_at"][..]),
        ("claimed_at", &["created_at"][..]),
        ("completed_at", &["created_at", "claimed_at"][..]),
    ] {
        let mut row = delivery.clone();
        for earlier in earlier_fields {
            row[*earlier] = before_zero.clone();
        }
        assert!(v018_accepts_mail_intake_failure(&serde_json::from_value(row.clone()).unwrap()));
        row[field] = zero.clone();
        assert!(
            !v018_accepts_mail_intake_failure(&serde_json::from_value(row).unwrap()),
            "v0.1.8 应拒绝 {field} 的零值时间"
        );
    }
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
fn decodes_http_sent_proof_details_with_required_nullable_id_lists() {
    let fixtures: [Value; 3] = serde_json::from_str(include_str!(
        "../../../../auction-contracts/tests/fixtures/admin/delivery-sent-proof-detail.json"
    ))
    .unwrap();
    let decode = |fixture| {
        decode_detail_with_actions(serde_json::from_value(fixture).unwrap(), DELIVERY_ID, true)
    };
    let [attached, historical, collected] =
        fixtures.clone().map(|fixture| decode(fixture).expect("后端实际详情投影"));
    let ids = ["300753957"];
    assert_eq!(attached.detail.evidence[0].remaining_equipment_ids().unwrap(), ids);
    assert_eq!(attached.detail.evidence[0].pre_collection_equipment_ids, Some(None));
    assert_eq!(historical.detail.evidence[0].remaining_equipment_ids, Some(None));
    assert_eq!(historical.detail.evidence[0].pre_collection_equipment_ids, Some(None));
    assert!(collected.detail.evidence[0].remaining_equipment_ids().unwrap().is_empty());
    assert_eq!(collected.detail.evidence[0].pre_collection_equipment_ids().unwrap(), ids);

    for field in ["remaining_equipment_ids", "pre_collection_equipment_ids"] {
        let mut missing = fixtures[1].clone();
        missing["data"]["detail"]["evidence"][0].as_object_mut().unwrap().remove(field);
        assert!(decode(missing).is_err(), "扩展投影必须包含 {field}");
    }
    let mut malformed = fixtures[0].clone();
    malformed["data"]["detail"]["evidence"][0]["remaining_equipment_ids"] = json!(["0300753957"]);
    assert!(decode(malformed).is_err(), "装备 ID 必须是规范正十进制");
    let mut extra = fixtures[0].clone();
    extra["data"]["detail"]["evidence"][0]["future_field"] = Value::Null;
    assert!(
        serde_json::from_value::<
            StrictDeliveryEnvelope<auction_contracts::AdminDeliveryDetailWithActionsResponse>,
        >(extra)
        .is_err()
    );
    assert!(
        decode_detail_with_actions(
            serde_json::from_value(fixtures[0].clone()).unwrap(),
            DELIVERY_ID,
            false,
        )
        .is_err()
    );
}

#[test]
fn decodes_http_sent_proof_preview_with_both_responsibility_reasons() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../auction-contracts/tests/fixtures/admin/delivery-sent-proof-preview.json"
    ))
    .unwrap();
    let decode = |fixture| decode_preview(serde_json::from_value(fixture).unwrap(), DELIVERY_ID);
    let decoded = decode(fixture.clone()).expect("后端实际预览投影");
    assert_eq!(decoded.prior_unbound_attempts, Some(true));
    assert_eq!(decoded.sent_before_current_attempt, Some(true));
    for field in ["prior_unbound_attempts", "sent_before_current_attempt"] {
        let mut missing = fixture.clone();
        missing["data"].as_object_mut().unwrap().remove(field);
        assert!(decode(missing).is_err(), "扩展预览必须包含 {field}");
    }
    let mut inconsistent = fixture;
    inconsistent["data"]["requires_attempt_history_acceptance"] = json!(false);
    assert!(decode(inconsistent).is_err(), "旧责任位必须等于两项原因的并集");
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
            prior_unbound_attempts: Some(false),
            sent_before_current_attempt: Some(false),
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

/// Sent 证据与当前 fence 的处置准入来自同一详情快照。
#[test]
fn detail_url_requests_sent_proof_and_disposition_actions() {
    assert_eq!(
        delivery_detail_url(DELIVERY_ID),
        format!("/api/v1/admin/deliveries/{DELIVERY_ID}?include=sent_proof")
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
