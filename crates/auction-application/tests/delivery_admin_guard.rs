//! Admin 投递动作守卫的动作迁移表与阻断项回归；只覆盖纯判定，不连接数据库。

use auction_application::delivery::{
    DeliveryAdminAction, DeliveryAdminError, DeliveryAdminGuardBlocker, DeliveryAdminGuardSnapshot,
    DeliveryAdminTransition,
};

fn snapshot(action: DeliveryAdminAction) -> DeliveryAdminGuardSnapshot {
    let status = action.transition().expected_status.to_owned();
    DeliveryAdminGuardSnapshot {
        action,
        expected_fencing_version: 7,
        fencing_version: 7,
        status,
        reason_code: None,
        origin_kind: "settlement".to_owned(),
        payload_operation: Some("send".to_owned()),
        sent_reference_sighted: false,
        funded_delivery: false,
        retired: false,
        lot_eligible: Some(true),
        has_reconcile_reference: true,
    }
}

#[test]
fn transition_table_is_the_only_state_mapping() {
    let cases = [
        (
            DeliveryAdminAction::Release,
            DeliveryAdminTransition { expected_status: "claimed", next_status: "pending" },
        ),
        (
            DeliveryAdminAction::RetryFailed,
            DeliveryAdminTransition { expected_status: "failed", next_status: "pending" },
        ),
        (
            DeliveryAdminAction::ReconcileRetry,
            DeliveryAdminTransition { expected_status: "manual_reconcile", next_status: "pending" },
        ),
        (
            DeliveryAdminAction::ReconcileSucceeded,
            DeliveryAdminTransition {
                expected_status: "manual_reconcile",
                next_status: "succeeded",
            },
        ),
        (
            DeliveryAdminAction::ReconcileFailed,
            DeliveryAdminTransition { expected_status: "manual_reconcile", next_status: "failed" },
        ),
    ];
    for (action, transition) in cases {
        assert_eq!(action.transition(), transition);
    }
}

#[test]
fn release_allows_only_claimed_and_clears_outcome_metadata() {
    let decision = snapshot(DeliveryAdminAction::Release).evaluate();
    assert!(decision.eligible());
    assert_eq!(decision.transition.next_status, "pending");
    assert_eq!(decision.result_metadata, serde_json::json!({}));
}

#[test]
fn funded_retired_and_stale_deliveries_block_in_fixed_order() {
    let mut guard = snapshot(DeliveryAdminAction::Release);
    guard.funded_delivery = true;
    guard.retired = true;
    guard.fencing_version = 6;
    let decision = guard.evaluate();
    assert_eq!(decision.blockers.first(), Some(&DeliveryAdminGuardBlocker::FundedDelivery));
    assert_eq!(DeliveryAdminGuardBlocker::FundedDelivery.error(), DeliveryAdminError::Invalid);

    let mut guard = snapshot(DeliveryAdminAction::Release);
    guard.retired = true;
    assert_eq!(
        guard.evaluate().blockers.first(),
        Some(&DeliveryAdminGuardBlocker::RetiredDelivery)
    );

    let mut guard = snapshot(DeliveryAdminAction::RetryFailed);
    guard.fencing_version = 8;
    assert_eq!(
        guard.evaluate().blockers.first(),
        Some(&DeliveryAdminGuardBlocker::StaleFencingVersion)
    );
    assert_eq!(DeliveryAdminGuardBlocker::StaleFencingVersion.error(), DeliveryAdminError::Stale);
}

#[test]
fn sent_sighting_blocks_reconcile_retry_and_failure_only_for_settlement_send() {
    let mut guard = snapshot(DeliveryAdminAction::ReconcileRetry);
    guard.sent_reference_sighted = true;
    assert_eq!(guard.evaluate().blockers.first(), Some(&DeliveryAdminGuardBlocker::SentSighted));
    assert_eq!(DeliveryAdminGuardBlocker::SentSighted.error(), DeliveryAdminError::SentSighted);

    let mut guard = snapshot(DeliveryAdminAction::ReconcileFailed);
    guard.sent_reference_sighted = true;
    assert!(guard.evaluate().blockers.contains(&DeliveryAdminGuardBlocker::SentSighted));

    // 非结算来源与 Release 不读取目击标记，保持旧判定范围。
    let mut guard = snapshot(DeliveryAdminAction::ReconcileRetry);
    guard.origin_kind = "manual".to_owned();
    guard.sent_reference_sighted = true;
    assert!(guard.evaluate().eligible());

    let mut guard = snapshot(DeliveryAdminAction::ReconcileRetry);
    guard.payload_operation = Some("return".to_owned());
    guard.sent_reference_sighted = true;
    assert!(guard.evaluate().eligible());

    let mut guard = snapshot(DeliveryAdminAction::Release);
    guard.sent_reference_sighted = true;
    assert!(guard.evaluate().eligible());
}

#[test]
fn admin_cannot_reconcile_a_send_as_succeeded() {
    let decision = snapshot(DeliveryAdminAction::ReconcileSucceeded).evaluate();
    assert_eq!(
        decision.blockers.first(),
        Some(&DeliveryAdminGuardBlocker::ReconcileSucceededForSend)
    );
    assert_eq!(
        DeliveryAdminGuardBlocker::ReconcileSucceededForSend.error(),
        DeliveryAdminError::Invalid
    );

    let mut guard = snapshot(DeliveryAdminAction::ReconcileSucceeded);
    guard.payload_operation = None;
    let decision = guard.evaluate();
    assert!(decision.eligible());
    assert_eq!(
        decision.result_metadata,
        serde_json::json!({"resolution":"admin_reconciled_succeeded"})
    );

    let mut guard = snapshot(DeliveryAdminAction::ReconcileSucceeded);
    guard.payload_operation = None;
    guard.has_reconcile_reference = false;
    assert_eq!(
        guard.evaluate().blockers.first(),
        Some(&DeliveryAdminGuardBlocker::MissingReconcileReference)
    );
}

#[test]
fn lot_eligibility_precedes_status_mismatch() {
    let mut guard = snapshot(DeliveryAdminAction::RetryFailed);
    guard.lot_eligible = Some(false);
    guard.status = "manual_reconcile".to_owned();
    let decision = guard.evaluate();
    assert_eq!(
        decision.blockers,
        vec![
            DeliveryAdminGuardBlocker::DeliveryNotEligible,
            DeliveryAdminGuardBlocker::StatusMismatch
        ]
    );
    assert_eq!(DeliveryAdminGuardBlocker::DeliveryNotEligible.error(), DeliveryAdminError::Stale);

    // 对账动作不读取 lot 资格；缺失值不构成阻断。
    let mut guard = snapshot(DeliveryAdminAction::ReconcileFailed);
    guard.lot_eligible = None;
    assert!(guard.evaluate().eligible());
}

#[test]
fn quarantined_mail_can_only_be_terminated_as_failed() {
    let mut guard = snapshot(DeliveryAdminAction::Release);
    guard.reason_code = Some("mail_disposition_conflict".to_owned());
    assert_eq!(
        guard.evaluate().blockers.first(),
        Some(&DeliveryAdminGuardBlocker::QuarantinedMailActionNotAllowed)
    );

    let mut guard = snapshot(DeliveryAdminAction::ReconcileFailed);
    guard.reason_code = Some("mail_disposition_conflict".to_owned());
    let decision = guard.evaluate();
    assert!(decision.eligible());
    assert_eq!(
        decision.result_metadata,
        serde_json::json!({
            "resolution": "admin_reconciled_failed",
            "reason_code": "mail_disposition_conflict",
            "preflight_blocked": true,
            "outcome_unknown": false,
        })
    );

    let mut guard = snapshot(DeliveryAdminAction::ReconcileFailed);
    guard.reason_code = Some("write_unknown".to_owned());
    assert_eq!(
        guard.evaluate().result_metadata,
        serde_json::json!({"resolution":"admin_reconciled_failed"})
    );
}

#[test]
fn status_mismatch_is_stale_and_blocks_every_action() {
    let mut guard = snapshot(DeliveryAdminAction::Release);
    guard.status = "pending".to_owned();
    let decision = guard.evaluate();
    assert_eq!(decision.blockers.first(), Some(&DeliveryAdminGuardBlocker::StatusMismatch));
    assert_eq!(DeliveryAdminGuardBlocker::StatusMismatch.error(), DeliveryAdminError::Stale);
}
