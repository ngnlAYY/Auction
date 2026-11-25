//! 派发轮次 / 尝试 / 退件接管的冻结向量与核心判定。
//!
//! ID 与摘要向量由独立实现（Python 的 SHA-256 / SHA-1 长度前缀算法）核算；namespace、字节序或
//! 参数顺序变化必须让本文件失败。v5 字节一旦冻结不得再改。

use auction_domain::SettlementWorld;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{
    EnvelopeCapabilities, EnvelopeDecision, EnvelopeObservation, EnvelopeRoute, LegacyAuctionOwner,
    LockedEnvelopeFacts, ProofValidatorStatus, QuarantineReason, SenderIdentity,
    TrustedConsignmentRts, decide_envelope, decide_envelope_with_origin,
};
use auction_domain::consignment::dispatch::{
    AttemptKey, DispatchClaimFacts, DispatchRoundKey, DispatchRuleError, FrozenDispatchIdentity,
    RoundOrigin, WriteStartDecision, WriteStartFacts, WriteStartRejection, decide_write_start,
    dispatch_attempt_id, dispatch_ref, dispatch_round_id, next_round_number,
};
use auction_domain::consignment::returns::{
    AcquireConflict, AcquirePlan, AcquireQuarantine, ClaimFacts, ConsumeCommandRef,
    ConsumeConflict, ConsumeDecision, ConsumeFacts, ExistingConsumption, ExistingIntake,
    KFinalDisposition, LockedAcquireFacts, ReconcileReason, ReturnIntakeFacts, ReturnLeg,
    RtsOrigin, StartedRef, TakeProofFacts, TakeStartDecision, TakeStartFacts, TakeStartRejection,
    TakeStartRequest, decide_acquire_return, decide_consumption, decide_take_start,
    return_intake_id, start_return_take_command_id,
};
use auction_time::Timestamp;
use uuid::Uuid;

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("测试 UUID")
}

fn at(day: u32) -> Timestamp {
    Timestamp::from_ymd_hms(2026, 11, day, 0, 0, 0).unwrap()
}

fn round_key(round: u32) -> DispatchRoundKey {
    DispatchRoundKey {
        contract_id: uuid("11111111-1111-4111-8111-111111111111"),
        obligation_id: uuid("22222222-2222-4222-8222-222222222222"),
        round,
    }
}

#[test]
fn round_and_attempt_ids_match_frozen_vectors() {
    assert_eq!(dispatch_round_id(&round_key(1)), uuid("11be985f-d8c2-58af-a4d4-5d73f602b4e2"));
    assert_eq!(dispatch_round_id(&round_key(2)), uuid("9bb68330-1483-5b65-897c-030b2430f1b0"));
    // 模板里的轮引用是轮 ID 的小写带连字符文本（裁定 R5-C7）。
    assert_eq!(dispatch_ref(&round_key(1)), "11be985f-d8c2-58af-a4d4-5d73f602b4e2");
    let delivery = uuid("33333333-3333-4333-8333-333333333333");
    assert_eq!(
        dispatch_attempt_id(&AttemptKey { delivery_id: delivery, fence: 1 }),
        uuid("21d6c91b-c655-592e-ab29-2a080dc746f2")
    );
    assert_eq!(
        dispatch_attempt_id(&AttemptKey { delivery_id: delivery, fence: 2 }),
        uuid("185c1475-fe28-51ad-a5b8-75defe8dba91")
    );
    // 反例：合同、义务、轮次任一不同，轮 ID 必变。
    let base = dispatch_round_id(&round_key(1));
    let mut other = round_key(1);
    other.contract_id = uuid("33333333-3333-4333-8333-333333333333");
    assert_ne!(base, dispatch_round_id(&other));
    let mut other = round_key(1);
    other.obligation_id = other.contract_id;
    assert_ne!(base, dispatch_round_id(&other));
}

#[test]
fn return_intake_id_and_take_command_id_match_frozen_vectors() {
    let main = OriginalMailKey::new("42", SettlementWorld::Main, "100").unwrap();
    let isekai = OriginalMailKey::new("42", SettlementWorld::Isekai, "9007199254740993").unwrap();
    assert_eq!(return_intake_id(&main), uuid("e65ce243-f16e-5b82-8c3c-9ecc0937cb8e"));
    assert_eq!(return_intake_id(&isekai), uuid("fab5c23d-87c2-50f8-b387-816c4928f1a0"));
    let intake = uuid("44444444-4444-4444-8444-444444444444");
    let work = uuid("55555555-5555-4555-8555-555555555555");
    let attempt = uuid("66666666-6666-4666-8666-666666666666");
    assert_eq!(
        start_return_take_command_id(intake, work, attempt, 3).as_uuid(),
        uuid("e1b361c6-573d-51aa-b55e-849f848de37b")
    );
    // 反例：fence 或 attempt 变化，命令 ID 必变（过租后的新尝试不占旧尝试的 ID）。
    assert_eq!(
        start_return_take_command_id(intake, work, attempt, 4).as_uuid(),
        uuid("ffd55984-8461-5c7d-beed-7ffbdc3e1ca9")
    );
    assert_ne!(
        start_return_take_command_id(intake, work, work, 3),
        start_return_take_command_id(intake, work, attempt, 3)
    );
}

fn frozen() -> FrozenDispatchIdentity {
    FrozenDispatchIdentity {
        rights_holder_identity: "buyer-1".to_owned(),
        bot_uid: "42".to_owned(),
        world: SettlementWorld::Main,
        subject: "[consign] x".to_owned(),
        template: "original".to_owned(),
        body_digest: [7; 32],
        round_ref: dispatch_ref(&round_key(1)),
        d0: at(1),
        credits: 0,
        cod: 101,
        equipment_ids: vec!["900".to_owned(), "12".to_owned()],
    }
}

fn hex(bytes: [u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn frozen_identity_digest_matches_frozen_vector_and_detects_drift() {
    let identity = frozen();
    identity.validate().unwrap();
    assert_eq!(
        hex(identity.identity_digest()),
        "81410077b58b8b768a4e27d161ed7006b8c42c54c9a263ee66589feabd5141c1"
    );
    // 装备入参顺序不影响摘要；任何一个冻结字段变化都改变摘要，起写后重启据此失败关闭。
    let mut reordered = frozen();
    reordered.equipment_ids.reverse();
    assert_eq!(reordered.identity_digest(), identity.identity_digest());
    let mut drift = frozen();
    drift.cod = 102;
    assert_ne!(drift.identity_digest(), identity.identity_digest());
    let mut drift = frozen();
    drift.body_digest = [8; 32];
    assert_ne!(drift.identity_digest(), identity.identity_digest());
    // 反例：负金额、重复装备、空字段一律无效。
    let mut invalid = frozen();
    invalid.credits = -1;
    assert_eq!(invalid.validate(), Err(DispatchRuleError::InvalidIdentity));
    let mut invalid = frozen();
    invalid.equipment_ids = vec!["1".to_owned(), "1".to_owned()];
    assert_eq!(invalid.validate(), Err(DispatchRuleError::InvalidIdentity));
}

#[test]
fn round_numbers_advance_only_from_a_closed_prior_round() {
    let closed = RoundOrigin::Redispatch { closed_by_return_intake: Uuid::from_u128(9) };
    assert_eq!(next_round_number(None, &RoundOrigin::First), Ok(1));
    assert_eq!(next_round_number(Some(1), &closed), Ok(2));
    assert_eq!(
        next_round_number(Some(1), &RoundOrigin::First),
        Err(DispatchRuleError::FirstRoundExists)
    );
    assert_eq!(next_round_number(None, &closed), Err(DispatchRuleError::MissingPriorRound));
    let nil = RoundOrigin::Redispatch { closed_by_return_intake: Uuid::nil() };
    assert_eq!(next_round_number(Some(1), &nil), Err(DispatchRuleError::MissingClosure));
    assert_eq!(next_round_number(Some(u32::MAX), &closed), Err(DispatchRuleError::InvalidRound));
}

fn key(mid: &str) -> OriginalMailKey {
    OriginalMailKey::new("42", SettlementWorld::Main, mid).unwrap()
}

fn origin() -> RtsOrigin {
    RtsOrigin {
        contract_id: Uuid::from_u128(1),
        obligation_id: Uuid::from_u128(2),
        original_round: 1,
        original_attempt: AttemptKey { delivery_id: Uuid::from_u128(3), fence: 1 },
        bot_uid: "42".to_owned(),
        world: SettlementWorld::Main,
        sent_mid: "100".to_owned(),
        sent_at: at(1),
        leg: ReturnLeg::Original,
        frozen_manifest_digest: [9; 32],
        session_id: Uuid::from_u128(5),
    }
}

fn acquire_facts() -> LockedAcquireFacts {
    LockedAcquireFacts {
        k: key("200"),
        rts_observation_id: Uuid::from_u128(4),
        rts_returned_at: at(5),
        trusted_rts: true,
        origin: Some(origin()),
        observed_manifest_digest: Some([9; 32]),
        existing_for_k: None,
        k_final: KFinalDisposition::None,
        same_leg_other_k: None,
    }
}

#[test]
fn acquire_return_accepts_only_consistent_trusted_facts() {
    let AcquirePlan::Acquire(plan) = decide_acquire_return(&acquire_facts()) else {
        panic!("全部事实一致应接管");
    };
    assert_eq!(
        (plan.k.clone(), plan.original_round, plan.sent_mid.as_str()),
        (key("200"), 1, "100")
    );
}

#[test]
fn acquire_return_fails_closed_on_every_contradiction() {
    let quarantined = |mutate: fn(&mut LockedAcquireFacts), reason: AcquireQuarantine| {
        let mut facts = acquire_facts();
        mutate(&mut facts);
        assert_eq!(decide_acquire_return(&facts), AcquirePlan::Quarantine(reason));
    };
    quarantined(|f| f.trusted_rts = false, AcquireQuarantine::UntrustedRts);
    quarantined(|f| f.origin = None, AcquireQuarantine::OriginUnknown);
    // RTS 自身就是“原信”、世界或 Bot 不符、RTS 早于原发送、轮次为零：来源自相矛盾。
    quarantined(|f| f.k = key("100"), AcquireQuarantine::OriginContradiction);
    quarantined(
        |f| f.k = OriginalMailKey::new("42", SettlementWorld::Isekai, "200").unwrap(),
        AcquireQuarantine::OriginContradiction,
    );
    quarantined(
        |f| f.k = OriginalMailKey::new("43", SettlementWorld::Main, "200").unwrap(),
        AcquireQuarantine::OriginContradiction,
    );
    quarantined(
        |f| f.rts_returned_at = Timestamp::from_ymd_hms(2026, 10, 31, 0, 0, 0).unwrap(),
        AcquireQuarantine::OriginContradiction,
    );
    quarantined(
        |f| {
            if let Some(origin) = &mut f.origin {
                origin.original_round = 0;
            }
        },
        AcquireQuarantine::OriginContradiction,
    );
    // 清单摘要读不出或与冻结值不符。
    quarantined(|f| f.observed_manifest_digest = None, AcquireQuarantine::ManifestMismatch);
    quarantined(
        |f| f.observed_manifest_digest = Some([1; 32]),
        AcquireQuarantine::ManifestMismatch,
    );

    let conflict = |mutate: fn(&mut LockedAcquireFacts), reason: AcquireConflict| {
        let mut facts = acquire_facts();
        mutate(&mut facts);
        assert_eq!(decide_acquire_return(&facts), AcquirePlan::Conflict(reason));
    };
    conflict(|f| f.k_final = KFinalDisposition::Other, AcquireConflict::OppositeDisposition);
    conflict(|f| f.same_leg_other_k = Some(key("201")), AcquireConflict::SameLegSecondMid);
}

#[test]
fn acquire_return_replays_same_k_and_conflicts_when_facts_differ() {
    let existing = ExistingIntake {
        contract_id: Uuid::from_u128(1),
        obligation_id: Uuid::from_u128(2),
        original_round: 1,
        original_attempt: AttemptKey { delivery_id: Uuid::from_u128(3), fence: 1 },
        sent_mid: "100".to_owned(),
        leg: ReturnLeg::Original,
        frozen_manifest_digest: [9; 32],
    };
    let mut facts = acquire_facts();
    facts.existing_for_k = Some(existing.clone());
    facts.k_final = KFinalDisposition::AcquireReturn;
    facts.same_leg_other_k = Some(key("200"));
    assert_eq!(decide_acquire_return(&facts), AcquirePlan::Replay);
    // 同 K 重复到达但应退腿不同：冲突，不静默改写。
    if let Some(origin) = &mut facts.origin {
        origin.leg = ReturnLeg::Carrier;
    }
    assert_eq!(
        decide_acquire_return(&facts),
        AcquirePlan::Conflict(AcquireConflict::IntakeFactsDiffer)
    );
    // 同 K 同腿但原 Sent MID 或冻结清单摘要不同：同样是事实不同，冲突。
    let differs = |mutate: fn(&mut RtsOrigin)| {
        let mut facts = acquire_facts();
        facts.existing_for_k = Some(existing.clone());
        facts.k_final = KFinalDisposition::AcquireReturn;
        if let Some(origin) = &mut facts.origin {
            mutate(origin);
        }
        assert_eq!(
            decide_acquire_return(&facts),
            AcquirePlan::Conflict(AcquireConflict::IntakeFactsDiffer)
        );
    };
    differs(|origin| origin.sent_mid = "101".to_owned());
    differs(|origin| origin.frozen_manifest_digest = [8; 32]);
    // 不同的 RTS 观察 ID 不是不同的接管事实：仍是重放。
    let mut redelivered = acquire_facts();
    redelivered.existing_for_k = Some(existing);
    redelivered.k_final = KFinalDisposition::AcquireReturn;
    redelivered.rts_observation_id = Uuid::from_u128(44);
    assert_eq!(decide_acquire_return(&redelivered), AcquirePlan::Replay);
}

#[test]
fn acquire_return_fails_closed_when_final_slot_and_intake_disagree() {
    let existing = ExistingIntake {
        contract_id: Uuid::from_u128(1),
        obligation_id: Uuid::from_u128(2),
        original_round: 1,
        original_attempt: AttemptKey { delivery_id: Uuid::from_u128(3), fence: 1 },
        sent_mid: "100".to_owned(),
        leg: ReturnLeg::Original,
        frozen_manifest_digest: [9; 32],
    };
    // 终局已是 acquire_return 却没有 intake：不得再建一条。
    let mut orphan_final = acquire_facts();
    orphan_final.k_final = KFinalDisposition::AcquireReturn;
    assert_eq!(
        decide_acquire_return(&orphan_final),
        AcquirePlan::Conflict(AcquireConflict::InconsistentFinal)
    );
    // 有 intake 却没有终局：同样矛盾，不当作重放。
    let mut orphan_intake = acquire_facts();
    orphan_intake.existing_for_k = Some(existing);
    assert_eq!(
        decide_acquire_return(&orphan_intake),
        AcquirePlan::Conflict(AcquireConflict::InconsistentFinal)
    );
}

fn start_facts() -> TakeStartFacts {
    TakeStartFacts {
        intake_open: true,
        intake_start: None,
        work_start: None,
        opposite_action_present: false,
        claim: ClaimFacts {
            current_fence: 1,
            current_attempt_id: Uuid::from_u128(20),
            current_claim_token: Uuid::from_u128(21),
            lease_live: true,
            session_binding_digest: [5; 32],
            frozen_manifest_digest: [9; 32],
        },
        request: TakeStartRequest {
            attempt_id: Uuid::from_u128(20),
            fence: 1,
            claim_token: Uuid::from_u128(21),
            session_binding_digest: [5; 32],
            expected_manifest_digest: [9; 32],
        },
    }
}

#[test]
fn take_start_grants_first_only_when_no_start_exists() {
    assert_eq!(decide_take_start(&start_facts()), TakeStartDecision::First);
    // 已有成功起写：任何新领取（新 fence、新 attempt）都只得到“已提交”，没有许可。
    let started = StartedRef { attempt_id: Uuid::from_u128(10), fence: 1 };
    let mut renewed = start_facts();
    renewed.intake_start = Some(started);
    renewed.work_start = Some(started);
    renewed.claim.current_fence = 2;
    renewed.request.fence = 2;
    assert_eq!(decide_take_start(&renewed), TakeStartDecision::Already(started));
    // 矛盾：intake 与 work 的起写不一致或只有一边有，失败关闭。
    let mut inconsistent = start_facts();
    inconsistent.intake_start = Some(started);
    assert_eq!(
        decide_take_start(&inconsistent),
        TakeStartDecision::Reject(TakeStartRejection::StartedInconsistent)
    );
    inconsistent.work_start = Some(StartedRef { attempt_id: Uuid::from_u128(11), fence: 1 });
    assert_eq!(
        decide_take_start(&inconsistent),
        TakeStartDecision::Reject(TakeStartRejection::StartedInconsistent)
    );
}

#[test]
fn take_start_rejects_stale_claims() {
    let rejected = |mutate: fn(&mut TakeStartFacts), reason: TakeStartRejection| {
        let mut facts = start_facts();
        mutate(&mut facts);
        assert_eq!(decide_take_start(&facts), TakeStartDecision::Reject(reason));
    };
    rejected(|f| f.claim.current_fence = 2, TakeStartRejection::FenceMismatch);
    rejected(
        |f| f.claim.current_claim_token = Uuid::from_u128(99),
        TakeStartRejection::FenceMismatch,
    );
    rejected(|f| f.claim.lease_live = false, TakeStartRejection::LeaseLost);
    rejected(|f| f.claim.session_binding_digest = [6; 32], TakeStartRejection::SessionMismatch);
    rejected(
        |f| f.request.expected_manifest_digest = [8; 32],
        TakeStartRejection::ManifestMismatch,
    );
    rejected(|f| f.intake_open = false, TakeStartRejection::IntakeNotOpen);
    rejected(|f| f.opposite_action_present = true, TakeStartRejection::OppositeAction);
}

fn intake_facts(leg: ReturnLeg) -> ReturnIntakeFacts {
    ReturnIntakeFacts {
        k: key("200"),
        contract_id: Uuid::from_u128(1),
        obligation_id: Uuid::from_u128(2),
        original_round: 1,
        original_attempt: AttemptKey { delivery_id: Uuid::from_u128(3), fence: 1 },
        sent_mid: "100".to_owned(),
        rts_observation_id: Uuid::from_u128(4),
        rts_returned_at: at(5),
        leg,
        frozen_manifest_digest: [9; 32],
        session_id: Uuid::from_u128(5),
    }
}

fn consume_facts(leg: ReturnLeg) -> ConsumeFacts {
    let command = ConsumeCommandRef {
        attempt: StartedRef { attempt_id: Uuid::from_u128(20), fence: 1 },
        validator_version: "v1".to_owned(),
        proof_digest: [3; 32],
    };
    let started = StartedRef { attempt_id: Uuid::from_u128(20), fence: 1 };
    ConsumeFacts {
        intake_id: return_intake_id(&key("200")),
        intake: intake_facts(leg),
        started: Some(started),
        existing_consumption: None,
        leg_consumed_by_other: false,
        proof: Some(TakeProofFacts {
            proof_id: Uuid::from_u128(30),
            exact_take: true,
            validated: true,
            validator_status: ProofValidatorStatus::SignedOff,
            validator_version: "v1".to_owned(),
            proof_digest: [3; 32],
            k: key("200"),
            attempt: started,
        }),
        command,
    }
}

#[test]
fn consumption_requires_exact_take_proof_and_happens_once() {
    let ConsumeDecision::Consume(plan) = decide_consumption(&consume_facts(ReturnLeg::CashRefund))
    else {
        panic!("证据齐全应消费");
    };
    assert!(plan.funds_cash_return);
    let ConsumeDecision::Consume(plan) = decide_consumption(&consume_facts(ReturnLeg::Original))
    else {
        panic!("证据齐全应消费");
    };
    assert!(!plan.funds_cash_return);

    // 已消费：同 intake 同 K 只回放原结果；换 intake 或同腿被另一接管消费则冲突。
    let mut again = consume_facts(ReturnLeg::CashRefund);
    again.existing_consumption = Some(ExistingConsumption {
        intake_id: again.intake_id,
        k: key("200"),
        proof_id: Uuid::from_u128(30),
        proof_digest: [3; 32],
    });
    assert_eq!(
        decide_consumption(&again),
        ConsumeDecision::AlreadyConsumed { proof_id: Uuid::from_u128(30) }
    );
    again.existing_consumption = Some(ExistingConsumption {
        intake_id: Uuid::from_u128(77),
        k: key("200"),
        proof_id: Uuid::from_u128(30),
        proof_digest: [3; 32],
    });
    assert_eq!(
        decide_consumption(&again),
        ConsumeDecision::Conflict(ConsumeConflict::ConsumedByOther)
    );
    // 重放引用了不同的证明摘要：矛盾，不回放原结果。
    again.existing_consumption = Some(ExistingConsumption {
        intake_id: again.intake_id,
        k: key("200"),
        proof_id: Uuid::from_u128(30),
        proof_digest: [4; 32],
    });
    assert_eq!(
        decide_consumption(&again),
        ConsumeDecision::Conflict(ConsumeConflict::ProofMismatch)
    );
    let mut leg_taken = consume_facts(ReturnLeg::Original);
    leg_taken.leg_consumed_by_other = true;
    assert_eq!(
        decide_consumption(&leg_taken),
        ConsumeDecision::Conflict(ConsumeConflict::LegAlreadyConsumed)
    );
}

#[test]
fn consumption_reconciles_on_missing_evidence_and_conflicts_on_mismatch() {
    let mut facts = consume_facts(ReturnLeg::CashPayout);
    facts.proof = None;
    assert_eq!(
        decide_consumption(&facts),
        ConsumeDecision::Reconcile(ReconcileReason::EvidenceMissing)
    );
    let mut facts = consume_facts(ReturnLeg::CashPayout);
    if let Some(proof) = &mut facts.proof {
        proof.exact_take = false;
    }
    assert_eq!(
        decide_consumption(&facts),
        ConsumeDecision::Reconcile(ReconcileReason::EvidenceInsufficient)
    );
    let mut facts = consume_facts(ReturnLeg::CashPayout);
    facts.started = None;
    assert_eq!(decide_consumption(&facts), ConsumeDecision::Reconcile(ReconcileReason::NotStarted));
    // 证明指向别的 K 或别的 attempt：矛盾，不消费。
    let mut facts = consume_facts(ReturnLeg::CashPayout);
    if let Some(proof) = &mut facts.proof {
        proof.k = key("999");
    }
    assert_eq!(
        decide_consumption(&facts),
        ConsumeDecision::Conflict(ConsumeConflict::ProofMismatch)
    );
    let mut facts = consume_facts(ReturnLeg::CashPayout);
    if let Some(proof) = &mut facts.proof {
        proof.attempt.fence = 2;
    }
    assert_eq!(
        decide_consumption(&facts),
        ConsumeDecision::Conflict(ConsumeConflict::ProofMismatch)
    );
}

#[test]
fn consumption_requires_a_signed_validator_and_matching_command_references() {
    // 验证器未签收（证据本身完整、精确 Take）：只核对，不消费。
    let mut unsigned = consume_facts(ReturnLeg::CashPayout);
    if let Some(proof) = &mut unsigned.proof {
        proof.validator_status = ProofValidatorStatus::Unsigned;
    }
    assert_eq!(
        decide_consumption(&unsigned),
        ConsumeDecision::Reconcile(ReconcileReason::EvidenceInsufficient)
    );
    // 证据不完整（validated = false）：只核对，不消费。
    let mut incomplete = consume_facts(ReturnLeg::CashPayout);
    if let Some(proof) = &mut incomplete.proof {
        proof.validated = false;
    }
    assert_eq!(
        decide_consumption(&incomplete),
        ConsumeDecision::Reconcile(ReconcileReason::EvidenceInsufficient)
    );
    // 命令自称的 attempt / fence 与实际起写不一致：矛盾，失败关闭。
    let mismatched = |mutate: fn(&mut ConsumeFacts)| {
        let mut facts = consume_facts(ReturnLeg::CashPayout);
        mutate(&mut facts);
        assert_eq!(
            decide_consumption(&facts),
            ConsumeDecision::Conflict(ConsumeConflict::ProofMismatch)
        );
    };
    mismatched(|f| f.command.attempt.fence = 2);
    mismatched(|f| f.command.attempt.attempt_id = Uuid::from_u128(21));
    // 命令的验证器版本或证明摘要与证明行不一致：同样失败关闭。
    mismatched(|f| f.command.validator_version = "v2".to_owned());
    mismatched(|f| f.command.proof_digest = [4; 32]);
    // 命令引用不符先于“证明缺失”：没有证明也不放过矛盾引用。
    let mut no_proof = consume_facts(ReturnLeg::CashPayout);
    no_proof.proof = None;
    no_proof.command.attempt.fence = 2;
    assert_eq!(
        decide_consumption(&no_proof),
        ConsumeDecision::Conflict(ConsumeConflict::ProofMismatch)
    );
}

#[test]
fn return_legs_map_to_cash_blocks_and_round_trip() {
    for leg in ReturnLeg::ALL {
        assert_eq!(ReturnLeg::parse(leg.as_str()), Ok(leg));
        assert_eq!(leg.is_cash(), leg.blocked_leg().is_some());
    }
    assert!(ReturnLeg::parse("cash").is_err());
}

fn envelope_facts(system_rts: bool) -> LockedEnvelopeFacts {
    LockedEnvelopeFacts {
        mail_key: Some(key("200")),
        observation: EnvelopeObservation {
            observation_id: Uuid::from_u128(1),
            session_binding_digest: [1; 32],
            manifest_digest: [2; 32],
            parser_version: "v1".to_owned(),
            subject_is_consign: false,
            sender_identity: SenderIdentity::Verified,
            list_detail_consistent: true,
            asset_count: Some(0),
            duplicate_raw_eid: false,
            canonical_alias_ambiguous: false,
            unknown_attachment: false,
            credits: Some(0),
            cod: Some(0),
            returnable: true,
            takeable: Some(true),
            system_rts,
            legacy_auction_owner: None,
            items: Vec::new(),
        },
        reservations: Vec::new(),
        envelope_seller_user_id: None,
        capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
        read_failure: false,
        already_decided: false,
        now: at(5),
    }
}

#[test]
fn trusted_consignment_rts_routes_to_acquire_return_only_with_an_origin() {
    let origin = TrustedConsignmentRts { obligation_id: Uuid::from_u128(2), original_round: 1 };
    let facts = envelope_facts(true);
    assert_eq!(
        decide_envelope_with_origin(&facts, Some(origin)),
        EnvelopeRoute::Consignment(EnvelopeDecision::AcquireReturn(origin))
    );
    // 无来源：与 decide_envelope 一致，仍隔离为疑似系统 RTS。
    let quarantined = EnvelopeRoute::Consignment(EnvelopeDecision::Quarantine(
        QuarantineReason::SuspectedSystemRts,
    ));
    assert_eq!(decide_envelope_with_origin(&facts, None), quarantined);
    assert_eq!(decide_envelope(&facts), quarantined);
    // 前置门仍先于接管：已有决定、读取失败时不得接管。
    let mut decided = envelope_facts(true);
    decided.already_decided = true;
    assert_eq!(decide_envelope_with_origin(&decided, Some(origin)), decide_envelope(&decided));
    let mut failed = envelope_facts(true);
    failed.read_failure = true;
    assert_eq!(decide_envelope_with_origin(&failed, Some(origin)), decide_envelope(&failed));
    // 旧拍卖 owner 先于接管：旧拍卖的系统 RTS 不被寄售接管，行为与既有判定逐字一致。
    let mut legacy = envelope_facts(true);
    legacy.observation.legacy_auction_owner = Some(LegacyAuctionOwner::SellerPayout);
    let routed = decide_envelope_with_origin(&legacy, Some(origin));
    assert_eq!(routed, decide_envelope(&legacy));
    assert!(matches!(routed, EnvelopeRoute::LegacyAuction(_)));
    // 不是 RTS 的整封不受来源影响。
    let plain = envelope_facts(false);
    assert_eq!(decide_envelope_with_origin(&plain, Some(origin)), decide_envelope(&plain));
}

fn write_start_facts() -> WriteStartFacts {
    WriteStartFacts {
        already_started: false,
        claim: DispatchClaimFacts {
            current_fence: 2,
            current_claim_token: Uuid::from_u128(60),
            lease_live: true,
        },
        request_fence: 2,
        request_claim_token: Uuid::from_u128(60),
    }
}

#[test]
fn dispatch_write_start_grants_first_only_to_the_live_current_claim() {
    assert_eq!(decide_write_start(&write_start_facts()), WriteStartDecision::First);
    let rejected = |mutate: fn(&mut WriteStartFacts), reason: WriteStartRejection| {
        let mut facts = write_start_facts();
        mutate(&mut facts);
        assert_eq!(decide_write_start(&facts), WriteStartDecision::Reject(reason));
    };
    rejected(|f| f.request_fence = 1, WriteStartRejection::FenceMismatch);
    rejected(|f| f.request_claim_token = Uuid::from_u128(61), WriteStartRejection::FenceMismatch);
    rejected(|f| f.claim.lease_live = false, WriteStartRejection::LeaseLost);
    // 已有成功起写：无论领取是否过期，都只得到“已提交”，没有许可。
    let mut started = write_start_facts();
    started.already_started = true;
    started.claim.lease_live = false;
    assert_eq!(decide_write_start(&started), WriteStartDecision::Already);
}
