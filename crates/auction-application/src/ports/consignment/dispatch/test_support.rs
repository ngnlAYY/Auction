//! 派发端口的内存假实现：只证明端口语义（幂等、轮次推进、冻结身份一致、起写只一次），不模拟 SQL。

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};

use auction_domain::CommandId;
use auction_domain::consignment::dispatch::{
    AttemptKey, DispatchClaimFacts, DispatchRoundKey, WriteStartDecision, WriteStartFacts,
    WriteStartRejection, decide_write_start, dispatch_attempt_id, dispatch_round_id,
    next_round_number,
};
use uuid::Uuid;

use super::commands::{
    AttemptReceipt, AuthorizeDispatch, EvidenceReceipt, MarkWriteStarted, RecordEvidence,
    RoundReceipt, StartAttempt,
};
use super::store::DispatchStore;
use crate::ports::consignment::deposit::{StartReceipt, WriteStartOutcome};
use crate::ports::consignment::errors::DepositBusinessRejection;
use crate::ports::consignment::errors::{ConsignmentError, ConsignmentFuture};
use crate::ports::core::RequestHash;

#[derive(Clone)]
enum Reply {
    Round(RoundReceipt),
    Attempt(AttemptReceipt),
    Started(StartReceipt),
    Evidence(EvidenceReceipt),
    Rejected(ConsignmentError),
}

struct RoundRow {
    key: DispatchRoundKey,
    delivery_id: Uuid,
    identity_digest: [u8; 32],
}

#[derive(Default)]
struct State {
    ledger: HashMap<CommandId, (RequestHash, Reply)>,
    rounds: HashMap<Uuid, RoundRow>,
    latest_round: HashMap<(Uuid, Uuid), u32>,
    closed_intakes: HashSet<Uuid>,
    attempts: HashMap<AttemptKey, Uuid>,
    /// 每个投递唯一的成功起写。
    started: HashMap<Uuid, StartReceipt>,
    /// 每个投递的当前领取（锁内重读的事实）。
    claims: HashMap<Uuid, DispatchClaimFacts>,
    evidence: HashSet<(AttemptKey, &'static str, String)>,
    first_commits: usize,
}

/// 派发端口假实现。
#[derive(Default)]
pub(super) struct FakeDispatch {
    state: Mutex<State>,
}

impl FakeDispatch {
    /// 预置一条已封闭上一轮的退件接管，供新轮引用。
    pub(super) fn close_round_by_intake(&self, intake: Uuid) {
        if let Ok(mut state) = self.state.lock() {
            state.closed_intakes.insert(intake);
        }
    }

    /// 设置某投递的当前领取。
    pub(super) fn set_claim(&self, delivery_id: Uuid, claim: DispatchClaimFacts) {
        if let Ok(mut state) = self.state.lock() {
            state.claims.insert(delivery_id, claim);
        }
    }

    pub(super) fn first_commits(&self) -> usize {
        self.state.lock().map_or(0, |state| state.first_commits)
    }

    pub(super) fn round_count(&self) -> usize {
        self.state.lock().map_or(0, |state| state.rounds.len())
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>, ConsignmentError> {
        let Ok(state) = self.state.lock() else {
            return Err(ConsignmentError::Unavailable);
        };
        Ok(state)
    }
}

/// 同命令 ID：同摘要回放原结果，异摘要冲突。
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

impl DispatchStore for FakeDispatch {
    fn authorize_dispatch(
        &self,
        command: AuthorizeDispatch,
    ) -> ConsignmentFuture<'_, RoundReceipt> {
        Box::pin(async move {
            let mut state = self.lock()?;
            if let Some(Reply::Round(mut receipt)) =
                replay(&state, command.command_id, command.request_hash)?
            {
                receipt.replayed = true;
                return Ok(receipt);
            }
            let prior = state
                .latest_round
                .get(&(command.key.contract_id, command.key.obligation_id))
                .copied();
            let Ok(round) = next_round_number(prior, &command.origin) else {
                return Err(DepositBusinessRejection::ActionNotAllowed.into());
            };
            if let auction_domain::consignment::dispatch::RoundOrigin::Redispatch {
                closed_by_return_intake,
            } = command.origin
                && !state.closed_intakes.contains(&closed_by_return_intake)
            {
                return Err(DepositBusinessRejection::EvidenceUnproven.into());
            }
            if command.frozen.validate().is_err() || command.key.round != round {
                return Err(DepositBusinessRejection::ActionNotAllowed.into());
            }
            let round_id = dispatch_round_id(&command.key);
            state.rounds.insert(
                round_id,
                RoundRow {
                    key: command.key,
                    delivery_id: command.delivery_id,
                    identity_digest: command.frozen.identity_digest(),
                },
            );
            state.latest_round.insert((command.key.contract_id, command.key.obligation_id), round);
            let receipt = RoundReceipt {
                command_id: command.command_id,
                round_id,
                round,
                delivery_id: command.delivery_id,
                replayed: false,
            };
            state.ledger.insert(command.command_id, (command.request_hash, Reply::Round(receipt)));
            Ok(receipt)
        })
    }

    fn start_attempt(&self, command: StartAttempt) -> ConsignmentFuture<'_, AttemptReceipt> {
        Box::pin(async move {
            let mut state = self.lock()?;
            if let Some(Reply::Attempt(mut receipt)) =
                replay(&state, command.command_id, command.request_hash)?
            {
                receipt.replayed = true;
                return Ok(receipt);
            }
            let row = state.rounds.get(&command.round_id).ok_or(ConsignmentError::Unavailable)?;
            // 冻结身份与轮不一致：失败关闭，不登记尝试。
            if row.delivery_id != command.attempt.delivery_id
                || row.identity_digest != command.frozen.identity_digest()
            {
                return Err(DepositBusinessRejection::SourceMismatch.into());
            }
            let attempt_id = dispatch_attempt_id(&command.attempt);
            state.attempts.insert(command.attempt, attempt_id);
            let receipt =
                AttemptReceipt { command_id: command.command_id, attempt_id, replayed: false };
            state
                .ledger
                .insert(command.command_id, (command.request_hash, Reply::Attempt(receipt)));
            Ok(receipt)
        })
    }

    fn mark_write_started(
        &self,
        command: MarkWriteStarted,
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
            let Some(attempt_id) = state.attempts.get(&command.attempt).copied() else {
                return Err(DepositBusinessRejection::ActionNotAllowed.into());
            };
            // 与存储一致：形状无效的凭据绑定不发许可（active 与版本由存储事务锁验）。
            if command.credential.as_ref().is_some_and(|binding| !binding.validate()) {
                return Err(DepositBusinessRejection::ActionNotAllowed.into());
            }
            let delivery_id = command.attempt.delivery_id;
            let claim = state
                .claims
                .get(&delivery_id)
                .copied()
                .ok_or(DepositBusinessRejection::FenceMismatch)?;
            // 每个投递最多一条成功起写：新 fence、新命令 ID 都只能得到“已提交”；
            // 首次许可必须来自当前领取。
            let existing = state.started.get(&delivery_id).cloned();
            let facts = WriteStartFacts {
                already_started: existing.is_some(),
                claim,
                request_fence: command.attempt.fence,
                request_claim_token: command.claim_token,
            };
            match decide_write_start(&facts) {
                WriteStartDecision::First => {}
                WriteStartDecision::Already => {
                    let existing = existing.ok_or(ConsignmentError::Unavailable)?;
                    // 与 `mark_write_started_in_tx` 一致：回放沿用已存 attempt/fence，但回执属于本次
                    // 命令；新命令也写下自己的成功回执，重放不再借用首个命令的身份。
                    let receipt = StartReceipt::started(
                        command.command_id,
                        existing.decision_id,
                        existing.work_id,
                        existing.attempt_id,
                        existing.fence,
                    );
                    state.ledger.insert(
                        command.command_id,
                        (command.request_hash, Reply::Started(receipt.clone())),
                    );
                    return Ok(WriteStartOutcome::AlreadyCommitted(receipt));
                }
                WriteStartDecision::Reject(rejection) => {
                    let error = match rejection {
                        WriteStartRejection::FenceMismatch => {
                            DepositBusinessRejection::FenceMismatch.into()
                        }
                        WriteStartRejection::LeaseLost => ConsignmentError::LeaseLost,
                    };
                    // 确定拒绝只占该次尝试的命令 ID。
                    state
                        .ledger
                        .insert(command.command_id, (command.request_hash, Reply::Rejected(error)));
                    return Err(error);
                }
            }
            let round_id = state
                .rounds
                .iter()
                .find(|(_, row)| row.delivery_id == delivery_id)
                .map(|(_, row)| dispatch_round_id(&row.key))
                .ok_or(ConsignmentError::Unavailable)?;
            let receipt = StartReceipt::started(
                command.command_id,
                round_id,
                delivery_id,
                attempt_id,
                command.attempt.fence,
            );
            state.started.insert(delivery_id, receipt.clone());
            state.first_commits += 1;
            state.ledger.insert(
                command.command_id,
                (command.request_hash, Reply::Started(receipt.clone())),
            );
            Ok(WriteStartOutcome::FirstCommitted(receipt))
        })
    }

    fn record_evidence(&self, command: RecordEvidence) -> ConsignmentFuture<'_, EvidenceReceipt> {
        Box::pin(async move {
            let mut state = self.lock()?;
            if let Some(Reply::Evidence(mut receipt)) =
                replay(&state, command.command_id, command.request_hash)?
            {
                receipt.replayed = true;
                return Ok(receipt);
            }
            if !state.attempts.contains_key(&command.attempt) {
                return Err(DepositBusinessRejection::ActionNotAllowed.into());
            }
            let key =
                (command.attempt, command.category.as_str(), command.remote_observation_id.clone());
            let replayed = !state.evidence.insert(key);
            let receipt = EvidenceReceipt { command_id: command.command_id, replayed };
            state
                .ledger
                .insert(command.command_id, (command.request_hash, Reply::Evidence(receipt)));
            Ok(receipt)
        })
    }
}
