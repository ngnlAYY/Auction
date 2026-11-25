//! D9 §5.1 六项授权矩阵的领域规则回归：资金动作与再售授权只在事实齐备时放行，
//! 矛盾或未知一律留案 / 留库 / 拒绝。

// 集成测试目标只含测试代码，断言直接解包。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use auction_domain::SettlementWorld;
use auction_domain::consignment::recovery::{
    BuyerRightEffect, DeliveryClosure, ListingEffect, PrincipalEffect, RecoveryDomainError,
    RefundEffect, RelistDecision, RelistEligibility, RelistFacts, RelistMode, ReminderDecision,
    ReminderFacts, ReminderStage, ReturnDispositionFacts, ReturnedLeg, decide_relist,
    decide_reminder, decide_return_disposition,
};
use auction_domain::consignment::{DepositError, HeldOffMarketReason, ListingState, SeasonState};

fn facts(leg: ReturnedLeg, world: SettlementWorld) -> ReturnDispositionFacts {
    ReturnDispositionFacts {
        leg,
        world,
        closure: DeliveryClosure::StillDeliverable,
        original_exactly_taken: true,
        principal_exactly_taken: true,
        unpaid_returns_reached_limit: false,
        opposite_payment_open: false,
        evidence_conflict: false,
        season_ended: false,
        original_payment_in_account: true,
    }
}

fn is_hold(f: &ReturnDispositionFacts) -> bool {
    let d = decide_return_disposition(f).unwrap();
    d.buyer_right == BuyerRightEffect::HoldForCase
        && d.refund == RefundEffect::None
        && d.principal == PrincipalEffect::None
        && d.listing == ListingEffect::Unchanged
        && d.relist == RelistEligibility::Forbidden
}

#[test]
fn main_unpaid_below_limit_retains_contract_and_forbids_relist() {
    let d =
        decide_return_disposition(&facts(ReturnedLeg::MainUnpaidOriginal, SettlementWorld::Main))
            .unwrap();
    assert_eq!(d.buyer_right, BuyerRightEffect::Retain);
    assert_eq!(d.relist, RelistEligibility::Forbidden);
    assert_eq!(d.refund, RefundEffect::None);
}

#[test]
fn main_unpaid_at_limit_relists_only_after_exact_take() {
    let mut f = facts(ReturnedLeg::MainUnpaidOriginal, SettlementWorld::Main);
    f.unpaid_returns_reached_limit = true;
    let d = decide_return_disposition(&f).unwrap();
    assert_eq!(d.buyer_right, BuyerRightEffect::CloseAtLimit);
    assert_eq!(d.listing, ListingEffect::HeldUnpaidRecovered);
    assert_eq!(d.relist, RelistEligibility::ManualOrAuto);
    f.original_exactly_taken = false;
    let d = decide_return_disposition(&f).unwrap();
    assert_eq!(d.buyer_right, BuyerRightEffect::CloseAtLimit);
    assert_eq!(d.listing, ListingEffect::Unchanged);
    assert_eq!(d.relist, RelistEligibility::Forbidden);
}

#[test]
fn isekai_unpaid_carrier_needs_both_takes_for_relist() {
    let mut f = facts(ReturnedLeg::IsekaiUnpaidCarrier, SettlementWorld::Isekai);
    let d = decide_return_disposition(&f).unwrap();
    assert_eq!(d.buyer_right, BuyerRightEffect::TerminateUnpaid);
    assert_eq!(d.principal, PrincipalEffect::RestoreOriginalCarrier);
    assert_eq!(d.relist, RelistEligibility::ManualOrAuto);
    f.principal_exactly_taken = false;
    let d = decide_return_disposition(&f).unwrap();
    assert_eq!(d.principal, PrincipalEffect::None);
    assert_eq!(d.listing, ListingEffect::Unchanged);
    assert_eq!(d.relist, RelistEligibility::Forbidden);
}

#[test]
fn isekai_carrier_after_close_writes_off_only_and_forbids_relist() {
    let mut f = facts(ReturnedLeg::IsekaiUnpaidCarrier, SettlementWorld::Isekai);
    f.closure = DeliveryClosure::ClosedBeforeRts;
    let d = decide_return_disposition(&f).unwrap();
    assert_eq!(d.buyer_right, BuyerRightEffect::TerminateCarrierLegOnly);
    assert_eq!(d.principal, PrincipalEffect::WriteOffOnly);
    assert_eq!(d.relist, RelistEligibility::Forbidden);
    // 本金未精确收回前不得核销。
    f.principal_exactly_taken = false;
    assert_eq!(decide_return_disposition(&f).unwrap().principal, PrincipalEffect::None);
}

#[test]
fn paid_handoff_refunds_p_only_after_exact_take_and_never_auto_relists() {
    let mut f = facts(ReturnedLeg::IsekaiPaidHandoff, SettlementWorld::Isekai);
    let d = decide_return_disposition(&f).unwrap();
    assert_eq!(d.buyer_right, BuyerRightEffect::ConvertToRefund);
    assert_eq!(d.refund, RefundEffect::RefundP);
    assert_eq!(d.listing, ListingEffect::HeldPaidReturned);
    assert_eq!(d.relist, RelistEligibility::ManualOnly);
    // 原件未精确收回：不退款。
    f.original_exactly_taken = false;
    let d = decide_return_disposition(&f).unwrap();
    assert_eq!(d.refund, RefundEffect::None);
    assert_eq!(d.buyer_right, BuyerRightEffect::Retain);
}

#[test]
fn paid_handoff_refund_requires_original_payment_in_account() {
    let mut f = facts(ReturnedLeg::IsekaiPaidHandoff, SettlementWorld::Isekai);
    f.original_payment_in_account = false;
    assert!(is_hold(&f));
    f.season_ended = true;
    assert!(is_hold(&f));
    f.original_payment_in_account = true;
    assert_eq!(decide_return_disposition(&f).unwrap().refund, RefundEffect::RefundP);
}

#[test]
fn paid_handoff_unclear_delivery_holds_before_refund() {
    let mut f = facts(ReturnedLeg::IsekaiPaidHandoff, SettlementWorld::Isekai);
    f.closure = DeliveryClosure::Unclear;
    assert!(is_hold(&f));
    f.season_ended = true;
    assert!(is_hold(&f));
}

#[test]
fn main_unclear_closure_never_relists_even_at_limit() {
    let mut f = facts(ReturnedLeg::MainUnpaidOriginal, SettlementWorld::Main);
    f.unpaid_returns_reached_limit = true;
    f.closure = DeliveryClosure::Unclear;
    assert!(is_hold(&f));
    f.closure = DeliveryClosure::ClosedBeforeRts;
    assert!(is_hold(&f));
}

#[test]
fn carrier_still_deliverable_with_season_ended_holds_for_case() {
    let mut f = facts(ReturnedLeg::IsekaiUnpaidCarrier, SettlementWorld::Isekai);
    f.season_ended = true;
    assert!(is_hold(&f));
}

#[test]
fn stale_epoch_rejects_manual_and_keeps_auto() {
    let mut m = relist_facts(RelistMode::Manual);
    m.epoch_current = false;
    assert_eq!(decide_relist(&m), RelistDecision::Reject(DepositError::ActionNotAllowed));
    let mut a = relist_facts(RelistMode::Auto { enabled_frozen: true });
    a.epoch_current = false;
    assert!(matches!(decide_relist(&a), RelistDecision::Keep { .. }));
    a.epoch_current = true;
    assert!(matches!(decide_relist(&a), RelistDecision::Relist { .. }));
}

#[test]
fn opposite_payment_open_blocks_every_money_move() {
    for (leg, world) in [
        (ReturnedLeg::MainUnpaidOriginal, SettlementWorld::Main),
        (ReturnedLeg::IsekaiUnpaidCarrier, SettlementWorld::Isekai),
        (ReturnedLeg::IsekaiPaidHandoff, SettlementWorld::Isekai),
    ] {
        let mut f = facts(leg, world);
        f.opposite_payment_open = true;
        assert!(is_hold(&f), "{leg:?}");
    }
}

#[test]
fn season_ended_refunds_p_without_relist_and_main_rejects_it() {
    let mut f = facts(ReturnedLeg::IsekaiPaidHandoff, SettlementWorld::Isekai);
    f.season_ended = true;
    f.original_exactly_taken = false;
    let d = decide_return_disposition(&f).unwrap();
    assert_eq!(d.refund, RefundEffect::RefundP);
    assert_eq!(d.relist, RelistEligibility::Forbidden);
    let mut m = facts(ReturnedLeg::MainUnpaidOriginal, SettlementWorld::Main);
    m.season_ended = true;
    assert_eq!(decide_return_disposition(&m), Err(RecoveryDomainError::SeasonEndedOnMain));
}

#[test]
fn passive_legs_never_relist_and_only_refund_restores_purpose() {
    for (leg, refund) in [
        (ReturnedLeg::BuyerRefund, RefundEffect::RestoreRefundPurpose),
        (ReturnedLeg::SellerPayout, RefundEffect::None),
        (ReturnedLeg::WithdrawalReturn, RefundEffect::None),
    ] {
        let d = decide_return_disposition(&facts(leg, SettlementWorld::Isekai)).unwrap();
        assert_eq!(d.buyer_right, BuyerRightEffect::NotApplicable);
        assert_eq!(d.refund, refund);
        assert_eq!(d.relist, RelistEligibility::Forbidden);
    }
}

#[test]
fn unclear_or_conflicting_evidence_holds_for_case() {
    let mut f = facts(ReturnedLeg::IsekaiUnpaidCarrier, SettlementWorld::Isekai);
    f.closure = DeliveryClosure::Unclear;
    assert!(is_hold(&f));
    let mut g = facts(ReturnedLeg::BuyerRefund, SettlementWorld::Main);
    g.evidence_conflict = true;
    assert!(is_hold(&g));
    // 腿与世界不符是装配错误，不猜测。
    let bad = facts(ReturnedLeg::IsekaiUnpaidCarrier, SettlementWorld::Main);
    assert_eq!(decide_return_disposition(&bad), Err(RecoveryDomainError::LegWorldMismatch));
}

fn relist_facts(mode: RelistMode) -> RelistFacts {
    RelistFacts {
        listing_state: ListingState::HeldOffMarket,
        held_reason: Some(HeldOffMarketReason::UnpaidRecovered),
        active_hold: false,
        relist: RelistEligibility::ManualOrAuto,
        seller_eligible: true,
        quota_available: true,
        service_enabled: true,
        season_state: SeasonState::Open,
        original_in_custody: true,
        predecessor_closed: true,
        unknown_dispatch_open: false,
        epoch_current: true,
        mode,
    }
}

#[test]
fn manual_relist_passes_and_each_unsafe_fact_rejects() {
    let ok = relist_facts(RelistMode::Manual);
    assert_eq!(decide_relist(&ok), RelistDecision::Relist { next_purpose: "relist" });
    let breakers: [fn(&mut RelistFacts); 9] = [
        |f| f.epoch_current = false,
        |f| f.active_hold = true,
        |f| f.seller_eligible = false,
        |f| f.quota_available = false,
        |f| f.season_state = SeasonState::Closed,
        |f| f.season_state = SeasonState::Unknown,
        |f| f.original_in_custody = false,
        |f| f.predecessor_closed = false,
        |f| f.unknown_dispatch_open = true,
    ];
    for breaker in breakers {
        let mut f = ok;
        breaker(&mut f);
        assert!(matches!(decide_relist(&f), RelistDecision::Reject(_)));
    }
    let mut f = ok;
    f.relist = RelistEligibility::Forbidden;
    assert_eq!(decide_relist(&f), RelistDecision::Reject(DepositError::ActionNotAllowed));
}

#[test]
fn auto_relist_reads_frozen_switch_and_never_applies_to_manual_only() {
    let on = relist_facts(RelistMode::Auto { enabled_frozen: true });
    assert_eq!(decide_relist(&on), RelistDecision::Relist { next_purpose: "relist" });
    let off = relist_facts(RelistMode::Auto { enabled_frozen: false });
    assert_eq!(
        decide_relist(&off),
        RelistDecision::Keep { reason: HeldOffMarketReason::UnpaidRecovered }
    );
    let mut paid = on;
    paid.relist = RelistEligibility::ManualOnly;
    assert!(matches!(decide_relist(&paid), RelistDecision::Reject(_)));
    let mut held = on;
    held.active_hold = true;
    assert_eq!(
        decide_relist(&held),
        RelistDecision::Keep { reason: HeldOffMarketReason::OperationsHold }
    );
}

fn reminder_facts(stage: ReminderStage) -> ReminderFacts {
    ReminderFacts {
        stage,
        due: true,
        shipment_proven: true,
        already_queued: false,
        paid: Some(false),
        cancelled: Some(false),
        round_current: true,
        dispatch_possibly_sent: false,
    }
}

#[test]
fn reminder_queues_once_and_voids_when_paid_or_possibly_sent() {
    let f = reminder_facts(ReminderStage::Scheduling);
    assert_eq!(decide_reminder(&f), ReminderDecision::Queue);
    assert_eq!(
        decide_reminder(&ReminderFacts { already_queued: true, ..f }),
        ReminderDecision::NotDue
    );
    let pre = reminder_facts(ReminderStage::PreDispatch);
    assert_eq!(
        decide_reminder(&ReminderFacts { paid: Some(true), ..pre }),
        ReminderDecision::VoidPaid
    );
    assert_eq!(
        decide_reminder(&ReminderFacts { dispatch_possibly_sent: true, paid: Some(true), ..pre }),
        ReminderDecision::NoResend
    );
    assert_eq!(decide_reminder(&ReminderFacts { paid: None, ..pre }), ReminderDecision::Withhold);
    assert_eq!(
        decide_reminder(&ReminderFacts { round_current: false, ..pre }),
        ReminderDecision::VoidRound
    );
    assert_eq!(
        decide_reminder(&ReminderFacts { shipment_proven: false, ..pre }),
        ReminderDecision::NotDue
    );
}
