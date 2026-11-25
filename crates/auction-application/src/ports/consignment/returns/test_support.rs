//! 退件接管端口的内存假实现：接管 / 起写 / 消费都走领域判定函数，只证明端口语义，不模拟 SQL。

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};

use auction_domain::CommandId;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::returns::{
    AcquirePlan, ClaimFacts, ConsumeCommandRef, ConsumeConflict, ConsumeDecision, ConsumeFacts,
    ExistingConsumption, ExistingIntake, KFinalDisposition, LockedAcquireFacts, ReturnIntakeFacts,
    RtsOrigin, StartedRef, TakeProofFacts, TakeStartDecision, TakeStartFacts, TakeStartRejection,
    TakeStartRequest, decide_acquire_return, decide_consumption, decide_take_start,
    return_intake_id,
};
use auction_time::Timestamp;
use uuid::Uuid;

use super::commands::{
    AcquireReceipt, AcquireReturn, ConsumeReceipt, ConsumeReturn, StartReturnTake,
};
use super::store::ReturnIntakeStore;
use crate::ports::consignment::deposit::{StartReceipt, WriteStartOutcome};
use crate::ports::consignment::errors::DepositBusinessRejection;
use crate::ports::consignment::errors::{ConsignmentError, ConsignmentFuture};
use crate::ports::core::RequestHash;

#[derive(Clone)]
enum Reply {
    Acquire(AcquireReceipt),
    Started(StartReceipt),
    Consumed(ConsumeReceipt),
    Rejected(ConsignmentError),
}

struct IntakeRow {
    facts: ReturnIntakeFacts,
    work_id: Uuid,
    decision_id: Uuid,
    open: bool,
}

/// 计数器：证明“只一次”的副作用次数。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Counters {
    pub intakes: usize,
    pub first_starts: usize,
    pub blocks_opened: usize,
    pub consumptions: usize,
    pub cash_returns: usize,
}

#[derive(Default)]
struct State {
    ledger: HashMap<CommandId, (RequestHash, Reply)>,
    intakes: HashMap<Uuid, IntakeRow>,
    by_leg: HashMap<(Uuid, Uuid, u32, &'static str), OriginalMailKey>,
    k_final: HashMap<OriginalMailKey, KFinalDisposition>,
    sent_at: HashMap<Uuid, Timestamp>,
    claims: HashMap<Uuid, ClaimFacts>,
    starts: HashMap<Uuid, StartedRef>,
    proofs: HashMap<Uuid, TakeProofFacts>,
    consumptions: HashMap<Uuid, (ExistingConsumption, ConsumeReceipt)>,
    leg_consumed: HashSet<(Uuid, Uuid, u32, &'static str)>,
    blocks: HashSet<Uuid>,
    counters: Counters,
    next_id: u128,
}

/// 退件接管端口假实现。
#[derive(Default)]
pub(super) struct FakeReturns {
    state: Mutex<State>,
}

impl FakeReturns {
    fn lock(&self) -> Result<MutexGuard<'_, State>, ConsignmentError> {
        let Ok(state) = self.state.lock() else {
            return Err(ConsignmentError::Unavailable);
        };
        Ok(state)
    }

    pub(super) fn counters(&self) -> Counters {
        self.state.lock().map_or_else(|_| Counters::default(), |state| state.counters)
    }

    pub(super) fn block_open(&self, intake_id: Uuid) -> bool {
        self.state.lock().is_ok_and(|state| state.blocks.contains(&intake_id))
    }

    /// 预置该 K 在原信最终处置槽里的既有结论。
    pub(super) fn seed_k_final(&self, k: &OriginalMailKey, value: KFinalDisposition) {
        if let Ok(mut state) = self.state.lock() {
            state.k_final.insert(k.clone(), value);
        }
    }

    /// 预置原发送时刻（锁内重读的事实）。
    pub(super) fn seed_sent_at(&self, obligation_id: Uuid, at: Timestamp) {
        if let Ok(mut state) = self.state.lock() {
            state.sent_at.insert(obligation_id, at);
        }
    }

    /// 设置某工作的当前领取。
    pub(super) fn set_claim(&self, work_id: Uuid, claim: ClaimFacts) {
        if let Ok(mut state) = self.state.lock() {
            state.claims.insert(work_id, claim);
        }
    }

    /// 预置观察到的 Take 证明。
    pub(super) fn seed_proof(&self, observation_id: Uuid, proof: TakeProofFacts) {
        if let Ok(mut state) = self.state.lock() {
            state.proofs.insert(observation_id, proof);
        }
    }
}

fn leg_key(facts: &ReturnIntakeFacts) -> (Uuid, Uuid, u32, &'static str) {
    (facts.contract_id, facts.obligation_id, facts.original_round, facts.leg.as_str())
}

fn replay(
    state: &State,
    command_id: CommandId,
    hash: RequestHash,
) -> Result<Option<Reply>, ConsignmentError> {
    match state.ledger.get(&command_id) {
        None => Ok(None),
        Some((stored, reply)) if *stored == hash => Ok(Some(reply.clone())),
        Some(_) => Err(ConsignmentError::CommandIdConflict),
    }
}

impl ReturnIntakeStore for FakeReturns {
    fn acquire_return(&self, command: AcquireReturn) -> ConsignmentFuture<'_, AcquireReceipt> {
        Box::pin(async move {
            let mut state = self.lock()?;
            if let Some(Reply::Acquire(mut receipt)) =
                replay(&state, command.command_id, command.request_hash)?
            {
                receipt.replayed = true;
                return Ok(receipt);
            }
            let facts = command.facts;
            let intake_id = return_intake_id(&facts.k);
            let existing_for_k = state.intakes.get(&intake_id).map(|row| ExistingIntake {
                contract_id: row.facts.contract_id,
                obligation_id: row.facts.obligation_id,
                original_round: row.facts.original_round,
                original_attempt: row.facts.original_attempt,
                sent_mid: row.facts.sent_mid.clone(),
                leg: row.facts.leg,
                frozen_manifest_digest: row.facts.frozen_manifest_digest,
            });
            let Some(sent_at) = state.sent_at.get(&facts.obligation_id).copied() else {
                return Err(DepositBusinessRejection::SourceMismatch.into());
            };
            let locked = LockedAcquireFacts {
                k: facts.k.clone(),
                rts_observation_id: facts.rts_observation_id,
                rts_returned_at: facts.rts_returned_at,
                trusted_rts: true,
                origin: Some(RtsOrigin {
                    contract_id: facts.contract_id,
                    obligation_id: facts.obligation_id,
                    original_round: facts.original_round,
                    original_attempt: facts.original_attempt,
                    bot_uid: facts.k.authenticated_bot_uid().to_owned(),
                    world: facts.k.world(),
                    sent_mid: facts.sent_mid.clone(),
                    sent_at,
                    leg: facts.leg,
                    frozen_manifest_digest: facts.frozen_manifest_digest,
                    session_id: facts.session_id,
                }),
                observed_manifest_digest: Some(facts.frozen_manifest_digest),
                existing_for_k,
                k_final: state.k_final.get(&facts.k).copied().unwrap_or(KFinalDisposition::None),
                same_leg_other_k: state.by_leg.get(&leg_key(&facts)).cloned(),
            };
            let receipt = match decide_acquire_return(&locked) {
                AcquirePlan::Acquire(plan) => {
                    state.next_id += 2;
                    let (work_id, decision_id) =
                        (Uuid::from_u128(state.next_id), Uuid::from_u128(state.next_id + 1));
                    state.by_leg.insert(leg_key(&plan), plan.k.clone());
                    state.k_final.insert(plan.k.clone(), KFinalDisposition::AcquireReturn);
                    state.intakes.insert(
                        intake_id,
                        IntakeRow { facts: *plan, work_id, decision_id, open: true },
                    );
                    state.counters.intakes += 1;
                    AcquireReceipt {
                        command_id: command.command_id,
                        intake_id,
                        work_id,
                        decision_id,
                        replayed: false,
                    }
                }
                AcquirePlan::Replay => {
                    let row = state.intakes.get(&intake_id).ok_or(ConsignmentError::Unavailable)?;
                    AcquireReceipt {
                        command_id: command.command_id,
                        intake_id,
                        work_id: row.work_id,
                        decision_id: row.decision_id,
                        replayed: true,
                    }
                }
                AcquirePlan::Quarantine(_) => {
                    return Err(DepositBusinessRejection::SourceMismatch.into());
                }
                AcquirePlan::Conflict(_) => {
                    return Err(DepositBusinessRejection::DispositionConflict.into());
                }
            };
            state
                .ledger
                .insert(command.command_id, (command.request_hash, Reply::Acquire(receipt)));
            Ok(receipt)
        })
    }

    fn start_return_take(
        &self,
        command: StartReturnTake,
    ) -> ConsignmentFuture<'_, WriteStartOutcome> {
        Box::pin(async move {
            let mut state = self.lock()?;
            match replay(&state, command.command_id, command.request_hash)? {
                Some(Reply::Started(receipt)) => {
                    return Ok(WriteStartOutcome::AlreadyCommitted(receipt));
                }
                Some(Reply::Rejected(error)) => return Err(error),
                _ => {}
            }
            let row = state.intakes.get(&command.intake_id).ok_or(ConsignmentError::Unavailable)?;
            if row.work_id != command.work_id {
                return Err(DepositBusinessRejection::SourceMismatch.into());
            }
            let (decision_id, leg, open) = (row.decision_id, row.facts.leg, row.open);
            let claim = state
                .claims
                .get(&command.work_id)
                .copied()
                .ok_or(DepositBusinessRejection::FenceMismatch)?;
            // intake 与 work 一一对应，两个槽读同一条起写事实。
            let started = state.starts.get(&command.intake_id).copied();
            let facts = TakeStartFacts {
                intake_open: open,
                intake_start: started,
                work_start: started,
                opposite_action_present: false,
                claim,
                request: TakeStartRequest {
                    attempt_id: command.attempt_id,
                    fence: command.fence,
                    claim_token: command.claim_token,
                    session_binding_digest: command.session_binding_digest,
                    expected_manifest_digest: command.expected_manifest_digest,
                },
            };
            let outcome = match decide_take_start(&facts) {
                TakeStartDecision::First => {
                    state.starts.insert(
                        command.intake_id,
                        StartedRef { attempt_id: command.attempt_id, fence: command.fence },
                    );
                    state.counters.first_starts += 1;
                    // 现金腿：同事务建立 UID opening 阻断，再许可 POST。
                    if leg.is_cash() && state.blocks.insert(command.intake_id) {
                        state.counters.blocks_opened += 1;
                    }
                    let receipt = StartReceipt::started(
                        command.command_id,
                        decision_id,
                        command.work_id,
                        command.attempt_id,
                        command.fence,
                    );
                    state.ledger.insert(
                        command.command_id,
                        (command.request_hash, Reply::Started(receipt.clone())),
                    );
                    return Ok(WriteStartOutcome::FirstCommitted(receipt));
                }
                TakeStartDecision::Already(original) => {
                    let receipt = StartReceipt::started(
                        command.command_id,
                        decision_id,
                        command.work_id,
                        original.attempt_id,
                        original.fence,
                    );
                    state.ledger.insert(
                        command.command_id,
                        (command.request_hash, Reply::Started(receipt.clone())),
                    );
                    return Ok(WriteStartOutcome::AlreadyCommitted(receipt));
                }
                TakeStartDecision::Reject(rejection) => rejection,
            };
            let error = match outcome {
                TakeStartRejection::FenceMismatch => DepositBusinessRejection::FenceMismatch.into(),
                TakeStartRejection::LeaseLost => ConsignmentError::LeaseLost,
                TakeStartRejection::OppositeAction
                | TakeStartRejection::StartedInconsistent
                | TakeStartRejection::IntakeNotOpen => {
                    DepositBusinessRejection::DispositionConflict.into()
                }
                TakeStartRejection::SessionMismatch | TakeStartRejection::ManifestMismatch => {
                    DepositBusinessRejection::SourceMismatch.into()
                }
            };
            // 确定拒绝只占该次尝试的命令 ID。
            state.ledger.insert(command.command_id, (command.request_hash, Reply::Rejected(error)));
            Err(error)
        })
    }

    fn consume_return(&self, command: ConsumeReturn) -> ConsignmentFuture<'_, ConsumeReceipt> {
        Box::pin(async move {
            let mut state = self.lock()?;
            match replay(&state, command.command_id, command.request_hash)? {
                Some(Reply::Consumed(mut receipt)) => {
                    receipt.replayed = true;
                    return Ok(receipt);
                }
                Some(Reply::Rejected(error)) => return Err(error),
                _ => {}
            }
            let row = state.intakes.get(&command.intake_id).ok_or(ConsignmentError::Unavailable)?;
            let intake = row.facts.clone();
            let facts = ConsumeFacts {
                intake_id: command.intake_id,
                intake: intake.clone(),
                started: state.starts.get(&command.intake_id).copied(),
                existing_consumption: state
                    .consumptions
                    .get(&command.intake_id)
                    .map(|(existing, _)| existing.clone()),
                leg_consumed_by_other: state.leg_consumed.contains(&leg_key(&intake))
                    && !state.consumptions.contains_key(&command.intake_id),
                proof: state.proofs.get(&command.observation_id).cloned(),
                command: ConsumeCommandRef {
                    attempt: StartedRef {
                        attempt_id: command.original_attempt_id,
                        fence: command.fence,
                    },
                    validator_version: command.validator_version.clone(),
                    proof_digest: command.proof_digest,
                },
            };
            let error = match decide_consumption(&facts) {
                ConsumeDecision::Consume(plan) => {
                    // 证明、消费与资金回补同事务；现金腿同时解除 UID 阻断。
                    let receipt = ConsumeReceipt {
                        command_id: command.command_id,
                        intake_id: plan.intake_id,
                        proof_id: plan.proof_id,
                        leg: plan.leg,
                        replayed: false,
                    };
                    state.consumptions.insert(
                        plan.intake_id,
                        (
                            ExistingConsumption {
                                intake_id: plan.intake_id,
                                k: plan.k.clone(),
                                proof_id: plan.proof_id,
                                proof_digest: command.proof_digest,
                            },
                            receipt,
                        ),
                    );
                    state.leg_consumed.insert(leg_key(&intake));
                    state.counters.consumptions += 1;
                    if plan.funds_cash_return {
                        state.counters.cash_returns += 1;
                        state.blocks.remove(&plan.intake_id);
                    }
                    if let Some(row) = state.intakes.get_mut(&plan.intake_id) {
                        row.open = false;
                    }
                    state.ledger.insert(
                        command.command_id,
                        (command.request_hash, Reply::Consumed(receipt)),
                    );
                    return Ok(receipt);
                }
                ConsumeDecision::AlreadyConsumed { .. } => {
                    let original = state
                        .consumptions
                        .get(&command.intake_id)
                        .map(|(_, original)| *original)
                        .ok_or(ConsignmentError::Unavailable)?;
                    let receipt = ConsumeReceipt { replayed: true, ..original };
                    state.ledger.insert(
                        command.command_id,
                        (command.request_hash, Reply::Consumed(receipt)),
                    );
                    return Ok(receipt);
                }
                // 证据不足不入账：之后补证可用新命令继续，保持 reconcile。
                ConsumeDecision::Reconcile(_) => {
                    return Err(DepositBusinessRejection::EvidenceUnproven.into());
                }
                ConsumeDecision::Conflict(ConsumeConflict::ProofMismatch) => {
                    DepositBusinessRejection::EvidenceConflict.into()
                }
                ConsumeDecision::Conflict(_) => {
                    DepositBusinessRejection::DispositionConflict.into()
                }
            };
            Err(error)
        })
    }
}
