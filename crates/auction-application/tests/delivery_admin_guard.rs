//! Admin 投递动作守卫的动作迁移表与阻断项回归；只覆盖纯判定，不连接数据库。

// 集成测试目标只含测试代码，断言直接解包。
#![allow(clippy::unwrap_used, clippy::expect_used)]

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
        has_sending_fence: true,
        origin_kind: "settlement".to_owned(),
        payload_operation: Some("send".to_owned()),
        sent_reference_sighted: false,
        funded_delivery: false,
        command_snapshot_readable: true,
        retired: false,
        lot_eligible: Some(true),
        settlement_eligible: Some(true),
        settlement_fulfillment_state: None,
        has_reconcile_reference: true,
        mail_handed_over: false,
        recipient_renamed: false,
        recovery_retry_eligible: true,
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
    assert!(decision.blockers.is_empty());
    assert_eq!(decision.transition.next_status, "pending");
    assert_eq!(decision.result_metadata, serde_json::json!({}));
}

#[test]
fn unreadable_command_snapshot_blocks_normal_disposition_actions() {
    use DeliveryAdminAction::{
        ReconcileFailed, ReconcileRetry, ReconcileSucceeded, Release, RetryFailed,
    };

    for action in [Release, RetryFailed, ReconcileRetry, ReconcileSucceeded, ReconcileFailed] {
        let mut guard = snapshot(action);
        guard.payload_operation = Some("return".to_owned());
        assert_eq!(guard.projected_blocker(action), None);
        guard.command_snapshot_readable = false;
        assert_eq!(
            guard.projected_blocker(action),
            Some(DeliveryAdminGuardBlocker::CommandSnapshotUnreadable)
        );
        assert_eq!(
            guard.evaluate().blockers.first().map(|blocker| blocker.error()),
            Some(DeliveryAdminError::CommandSnapshotUnreadable)
        );
        guard.funded_delivery = true;
        assert_eq!(
            guard.projected_blocker(action),
            Some(DeliveryAdminGuardBlocker::FundedDelivery)
        );
    }
}

#[test]
fn renamed_key_recipient_blocks_requeue_but_keeps_unknown_outcome_resolution() {
    let mut guard = snapshot(DeliveryAdminAction::RetryFailed);
    guard.origin_kind = "mail_intake".to_owned();
    assert_eq!(guard.projected_blocker(DeliveryAdminAction::RetryFailed), None);
    guard.recipient_renamed = true;
    assert_eq!(
        guard.projected_blocker(DeliveryAdminAction::RetryFailed),
        Some(DeliveryAdminGuardBlocker::RecipientRenamed)
    );
    guard.status = "manual_reconcile".to_owned();
    assert_eq!(
        guard.projected_blocker(DeliveryAdminAction::ReconcileRetry),
        Some(DeliveryAdminGuardBlocker::RecipientRenamed)
    );
    assert_eq!(guard.projected_blocker(DeliveryAdminAction::ReconcileFailed), None);
}

#[test]
fn recovery_gate_applies_only_to_retry_actions() {
    use DeliveryAdminAction::{ReconcileFailed, ReconcileRetry, Release, RetryFailed};

    for action in [Release, RetryFailed, ReconcileRetry, ReconcileFailed] {
        let mut guard = snapshot(action);
        guard.origin_kind = "settlement_recovery_action".to_owned();
        assert!(guard.projected_blocker(action).is_none());
        guard.recovery_retry_eligible = false;
        assert_eq!(
            guard.projected_blocker(action),
            matches!(action, RetryFailed | ReconcileRetry)
                .then_some(DeliveryAdminGuardBlocker::RecoveryRetryIneligible)
        );
    }
}

#[test]
fn linked_fulfillment_must_allow_the_action_before_delivery_is_changed() {
    use DeliveryAdminAction::{
        ReconcileFailed, ReconcileRetry, ReconcileSucceeded, Release, RetryFailed,
    };

    for action in [Release, RetryFailed, ReconcileRetry, ReconcileSucceeded, ReconcileFailed] {
        let mut guard = snapshot(action);
        guard.payload_operation = Some("return".to_owned());
        guard.settlement_fulfillment_state = Some("enqueued".to_owned());
        assert_eq!(
            guard.projected_blocker(action),
            (action != Release).then_some(DeliveryAdminGuardBlocker::SettlementFulfillmentState)
        );
        guard.settlement_fulfillment_state = Some("failed".to_owned());
        assert_eq!(
            guard.projected_blocker(action),
            matches!(action, ReconcileSucceeded | ReconcileFailed)
                .then_some(DeliveryAdminGuardBlocker::SettlementFulfillmentState)
        );
        guard.settlement_fulfillment_state = Some("manual_reconcile".to_owned());
        assert_eq!(guard.projected_blocker(action), None);
        guard.settlement_fulfillment_state = None;
        assert_eq!(guard.projected_blocker(action), None);
    }
}

#[test]
fn ordinary_reconciliation_requires_a_sending_fence() {
    use DeliveryAdminAction::{ReconcileFailed, ReconcileRetry, ReconcileSucceeded};

    let mut guard = snapshot(ReconcileFailed);
    guard.payload_operation = Some("return".to_owned());
    guard.has_sending_fence = false;
    for action in [ReconcileFailed, ReconcileSucceeded] {
        assert_eq!(
            guard.projected_blocker(action),
            Some(DeliveryAdminGuardBlocker::MissingSendingFence)
        );
    }
    assert_eq!(guard.projected_blocker(ReconcileRetry), None);
    guard.has_sending_fence = true;
    for action in [ReconcileFailed, ReconcileSucceeded] {
        assert_eq!(guard.projected_blocker(action), None);
    }
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
    assert!(guard.evaluate().blockers.is_empty());

    let mut guard = snapshot(DeliveryAdminAction::ReconcileRetry);
    guard.payload_operation = Some("return".to_owned());
    guard.sent_reference_sighted = true;
    assert!(guard.evaluate().blockers.is_empty());

    let mut guard = snapshot(DeliveryAdminAction::Release);
    guard.sent_reference_sighted = true;
    assert!(guard.evaluate().blockers.is_empty());
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
    assert!(decision.blockers.is_empty());
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
    assert!(guard.evaluate().blockers.is_empty());
}

/// 成交已撤销（或已不是当前派发）的结算投递不能被释放或重试重新激活：阻断优先于资金绑定与
/// 其余阻断，映射到专属错误；对账成功/失败不重新激活投递，不读取该资格。
#[test]
fn settlement_ineligibility_blocks_reactivation_first() {
    for action in [
        DeliveryAdminAction::Release,
        DeliveryAdminAction::RetryFailed,
        DeliveryAdminAction::ReconcileRetry,
    ] {
        let mut guard = snapshot(action);
        guard.settlement_eligible = Some(false);
        guard.funded_delivery = true;
        guard.fencing_version = 8;
        let decision = guard.evaluate();
        assert_eq!(
            decision.blockers.first(),
            Some(&DeliveryAdminGuardBlocker::SettlementIneligible),
            "{action:?}"
        );
        assert_eq!(decision.result_metadata, serde_json::json!({}));
    }
    assert_eq!(
        DeliveryAdminGuardBlocker::SettlementIneligible.error(),
        DeliveryAdminError::SettlementIneligible
    );

    // 非结算来源不读取该资格；缺失值不构成阻断。
    let mut guard = snapshot(DeliveryAdminAction::RetryFailed);
    guard.settlement_eligible = None;
    assert!(guard.evaluate().blockers.is_empty());

    let mut guard = snapshot(DeliveryAdminAction::ReconcileFailed);
    guard.settlement_eligible = Some(false);
    assert!(guard.evaluate().blockers.is_empty());
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
    guard.has_sending_fence = false;
    let decision = guard.evaluate();
    assert!(decision.blockers.is_empty());
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

#[test]
fn projection_requires_a_fence_that_can_be_submitted() {
    let action = DeliveryAdminAction::RetryFailed;
    let mut guard = snapshot(action);
    guard.fencing_version = 0;
    assert_eq!(
        guard.projected_blocker(action),
        Some(DeliveryAdminGuardBlocker::StaleFencingVersion)
    );
    guard.fencing_version = 1;
    assert_eq!(guard.projected_blocker(action), None);
}

/// 结算退件 Take 由退件案件推进：任何投递台动作都被拒绝，结果未知不能人工重试或声明结果。
#[test]
fn settlement_return_take_rejects_every_admin_action() {
    for action in [
        DeliveryAdminAction::Release,
        DeliveryAdminAction::RetryFailed,
        DeliveryAdminAction::ReconcileRetry,
        DeliveryAdminAction::ReconcileSucceeded,
        DeliveryAdminAction::ReconcileFailed,
    ] {
        let mut guard = snapshot(action);
        guard.origin_kind = "settlement_return_action".to_owned();
        guard.payload_operation = Some("take".to_owned());
        guard.settlement_eligible = None;
        let decision = guard.evaluate();
        assert_eq!(
            decision.blockers.first(),
            Some(&DeliveryAdminGuardBlocker::SettlementReturnActionOwned),
            "{action:?}"
        );
    }
    assert_eq!(
        DeliveryAdminGuardBlocker::SettlementReturnActionOwned.error(),
        DeliveryAdminError::Invalid
    );
}

/// 同一邮件已交接给结算退件 Take：原领取或退回意图的释放、重试与对账重试都被拒绝（普通重试不能
/// 绕过互斥），对账判成功或判失败等终结动作不受影响。
#[test]
fn handed_over_mail_intent_cannot_be_reactivated() {
    for action in [
        DeliveryAdminAction::Release,
        DeliveryAdminAction::RetryFailed,
        DeliveryAdminAction::ReconcileRetry,
    ] {
        let mut guard = snapshot(action);
        guard.origin_kind = "mail_intake".to_owned();
        guard.payload_operation = Some("take".to_owned());
        guard.settlement_eligible = None;
        guard.mail_handed_over = true;
        assert_eq!(
            guard.evaluate().blockers.first(),
            Some(&DeliveryAdminGuardBlocker::SettlementReturnActionOwned),
            "{action:?}"
        );
    }
    let mut failed = snapshot(DeliveryAdminAction::ReconcileFailed);
    failed.origin_kind = "mail_intake".to_owned();
    failed.payload_operation = Some("take".to_owned());
    failed.settlement_eligible = None;
    failed.mail_handed_over = true;
    assert!(failed.evaluate().blockers.is_empty(), "终结动作不复活意图");
}
