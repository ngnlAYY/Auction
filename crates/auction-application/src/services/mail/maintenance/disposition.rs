//! 寄售处置执行：领取持久 Take / Return 工作，只对首次起写提交执行一次 POST。
//!
//! 重放、`AlreadyCommitted`、起写失败与写后待证都只追加本地证据或保持核对；只有
//! `ConsignmentDispositionProof::into_signed()` 返回 `Some` 时才调用证明写端。

use std::sync::Arc;
use std::time::Instant;

use auction_time::Timestamp;
use uuid::Uuid;

use super::frozen_manifest::{
    ConsignmentDispositionRequest, ConsignmentNotSent, ConsignmentNotSentReason,
    DispositionObservationEvidence, DispositionReconcileReason, DispositionRetryReason,
    DispositionWorkClaim, disposition_retry_at,
};
use super::write_transport::{
    ConsignmentDispositionExecution, ConsignmentDispositionTransport, PreparedDisposition,
    SignedConsignmentProof,
};
use crate::ports::consignment::deposit::{MailProofKind, ReceiptDisposition, WriteStart};
use crate::ports::consignment::{ConsignmentError, ConsignmentFuture, DispositionAction};
use crate::services::consignment::{
    ApplyMailProofInput, DepositCommandService, StartDispositionWriteInput,
};

/// 处置工作仓储；实现方必须在数据库内核对 claim/fence/lease 后才允许完成或追加证据。
pub trait DispositionWorkStore: Send + Sync {
    /// 领取一条 due 的已决定 Take / Return 工作。
    fn claim_due(
        &self,
        worker_id: String,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> ConsignmentFuture<'_, Option<DispositionWorkClaim>>;

    /// 只读核对当前 claim/fence 仍有效；不写业务事实。
    fn check_current(&self, claim: &DispositionWorkClaim) -> ConsignmentFuture<'_, ()>;

    /// 写前安全释放给下一次领取；仍以原 token/fence 条件更新。
    ///
    /// `retry_at` 是持久到期时间，内存定时器只负责唤醒（不变量 5）；实现必须持久化它，
    /// 并在早于该时刻时不让 `claim_due` 返回本项。
    fn release_for_retry(
        &self,
        claim: &DispositionWorkClaim,
        reason: DispositionRetryReason,
        retry_at: Timestamp,
    ) -> ConsignmentFuture<'_, ()>;

    /// 追加只读证据，不授予任何处分权。
    fn record_observation(
        &self,
        claim: &DispositionWorkClaim,
        evidence: DispositionObservationEvidence,
    ) -> ConsignmentFuture<'_, ()>;

    /// 将工作留在核对态并追加可选证据；不得再排自动 POST。
    fn reconcile(
        &self,
        claim: &DispositionWorkClaim,
        reason: DispositionReconcileReason,
        evidence: Option<DispositionObservationEvidence>,
    ) -> ConsignmentFuture<'_, ()>;

    /// 只在证明已签收且应用成功时收口工作。
    fn complete(
        &self,
        claim: &DispositionWorkClaim,
        proof: &SignedConsignmentProof,
    ) -> ConsignmentFuture<'_, ()>;
}

/// 单轮处置请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispositionRoundRequest {
    /// Worker 实例名。
    pub worker_id: String,
    /// 锁后时钟。
    pub now: Timestamp,
    /// 本次租约截止。
    pub lease_until: Timestamp,
    /// 外部操作截止。
    pub deadline: Instant,
}

/// 单轮处置结果；只用于调度，不携带秘密。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispositionRound {
    /// 没有 due 工作。
    Idle,
    /// 证明已签收并应用。
    Applied,
    /// 留在核对态。
    Reconciled,
    /// 写前安全释放，稍后重试。
    Retried,
    /// 领取或围栏已失效。
    LeaseLost,
}

/// 处置执行用例。
pub struct DispositionMaintenanceService {
    commands: Arc<DepositCommandService>,
    work: Arc<dyn DispositionWorkStore>,
    transport: Arc<dyn ConsignmentDispositionTransport>,
}

impl DispositionMaintenanceService {
    /// 注入写用例、工作仓储与 4D1 处置适配器。
    pub fn new(
        commands: Arc<DepositCommandService>,
        work: Arc<dyn DispositionWorkStore>,
        transport: Arc<dyn ConsignmentDispositionTransport>,
    ) -> Self {
        Self { commands, work, transport }
    }

    /// 领取并处理一条工作；最多一次 POST。
    ///
    /// 不变量 4、5：只有首次起写提交能产生一次性许可；重放、旧 fence、提交未知和写后待证都
    /// 不会再发 POST，只追加证据并保持核对。
    pub async fn run_once(
        &self,
        request: DispositionRoundRequest,
    ) -> Result<DispositionRound, ConsignmentError> {
        let Some(claim) = self
            .work
            .claim_due(request.worker_id.clone(), request.now, request.lease_until)
            .await?
        else {
            return Ok(DispositionRound::Idle);
        };
        if !claim.validate() || request.now >= claim.lease_until {
            return Ok(DispositionRound::LeaseLost);
        }
        if self.work.check_current(&claim).await.is_err() {
            return Ok(DispositionRound::LeaseLost);
        }
        let prepared =
            match self.transport.prepare(disposition_request(&claim), request.deadline).await? {
                Ok(prepared) => prepared,
                Err(not_sent) => {
                    return self.handle_prepare_failure(&claim, not_sent, request.now).await;
                }
            };
        if prepared.observed_manifest_digest() != claim.manifest.manifest_digest {
            self.work
                .reconcile(
                    &claim,
                    DispositionReconcileReason::ManifestChanged,
                    Some(evidence_from_prepared(prepared.as_ref())),
                )
                .await?;
            return Ok(DispositionRound::Reconciled);
        }
        if self.work.check_current(&claim).await.is_err() {
            return Ok(DispositionRound::LeaseLost);
        }
        let session_binding_digest = prepared.session_binding_digest();
        let start = self
            .commands
            .start_disposition_write(StartDispositionWriteInput {
                final_decision_id: claim.final_decision_id,
                action: claim.action,
                delivery_id: claim.delivery_id,
                fence: claim.fence,
                work_id: claim.work_id,
                attempt_id: claim.attempt_id,
                claim_token: claim.claim_token,
                session_binding_digest,
                expected_manifest_digest: claim.manifest.manifest_digest,
            })
            .await;
        let permit = match start {
            Ok(WriteStart::FirstCommitted { receipt, permit }) => {
                if receipt.decision_id != claim.final_decision_id
                    || receipt.work_id != claim.work_id
                    || receipt.attempt_id != claim.attempt_id
                    || receipt.fence != claim.fence
                {
                    self.work
                        .reconcile(
                            &claim,
                            DispositionReconcileReason::StartRejected,
                            Some(evidence_from_prepared(prepared.as_ref())),
                        )
                        .await?;
                    return Ok(DispositionRound::Reconciled);
                }
                permit
            }
            Ok(WriteStart::AlreadyCommitted(_)) => {
                // 同 K 已有首决定：本轮不写第二份终局；下一轮由已有决定恢复。
                self.work
                    .reconcile(
                        &claim,
                        DispositionReconcileReason::AlreadyCommitted,
                        Some(evidence_from_prepared(prepared.as_ref())),
                    )
                    .await?;
                return Ok(DispositionRound::Reconciled);
            }
            Err(ConsignmentError::OutcomeUnknown) => {
                self.work
                    .reconcile(
                        &claim,
                        DispositionReconcileReason::StartOutcomeUnknown,
                        Some(evidence_from_prepared(prepared.as_ref())),
                    )
                    .await?;
                return Ok(DispositionRound::Reconciled);
            }
            Err(ConsignmentError::LeaseLost) => return Ok(DispositionRound::LeaseLost),
            Err(ConsignmentError::Unavailable) => {
                self.work
                    .reconcile(
                        &claim,
                        DispositionReconcileReason::StartOutcomeUnknown,
                        Some(evidence_from_prepared(prepared.as_ref())),
                    )
                    .await?;
                return Ok(DispositionRound::Reconciled);
            }
            Err(error) => {
                tracing::debug!(
                    code = error.api_error().map_or("consignment_unavailable", |(_, code)| code),
                    "寄售处置起写被确定拒绝"
                );
                self.work
                    .reconcile(
                        &claim,
                        DispositionReconcileReason::StartRejected,
                        Some(evidence_from_prepared(prepared.as_ref())),
                    )
                    .await?;
                return Ok(DispositionRound::Reconciled);
            }
        };
        let execution = self.transport.execute(prepared, permit, request.deadline).await?;
        match execution {
            ConsignmentDispositionExecution::NotSent { reason, observed_manifest_digest } => {
                // 起写已提交后许可已按值消费；即使确定未发出也不能再取得第二份许可。
                let evidence = observed_manifest_digest.map(|manifest_digest| {
                    DispositionObservationEvidence {
                        observation_id: local_evidence_id(&claim, manifest_digest),
                        manifest_digest,
                        session_binding_digest,
                    }
                });
                self.work.reconcile(&claim, reason.reconcile_reason(), evidence).await?;
                Ok(DispositionRound::Reconciled)
            }
            ConsignmentDispositionExecution::SentUnproven { reason } => {
                self.work.reconcile(&claim, reason.reconcile_reason(), None).await?;
                Ok(DispositionRound::Reconciled)
            }
            ConsignmentDispositionExecution::Proven(proof) => {
                let evidence = DispositionObservationEvidence {
                    observation_id: proof.observation_id(),
                    manifest_digest: claim.manifest.manifest_digest,
                    session_binding_digest,
                };
                let Some(signed) = proof.into_signed() else {
                    self.work.record_observation(&claim, evidence).await?;
                    self.work
                        .reconcile(&claim, DispositionReconcileReason::ProofUnsigned, None)
                        .await?;
                    return Ok(DispositionRound::Reconciled);
                };
                if signed.action() != claim.action {
                    self.work.record_observation(&claim, evidence).await?;
                    self.work
                        .reconcile(&claim, DispositionReconcileReason::ProofUnproven, None)
                        .await?;
                    return Ok(DispositionRound::Reconciled);
                }
                let receipt = self
                    .commands
                    .apply_mail_proof(ApplyMailProofInput {
                        original_attempt_id: claim.attempt_id,
                        proof_kind: proof_kind(claim.action),
                        final_decision_id: claim.final_decision_id,
                        action: claim.action,
                        delivery_id: claim.delivery_id,
                        fence: claim.fence,
                        observation_id: signed.observation_id(),
                        validator_version: signed.validator_version().to_owned(),
                        proof_digest: signed.proof_digest(),
                    })
                    .await;
                match receipt {
                    Ok(receipt) if proof_matches(claim.action, receipt.disposition) => {
                        self.work.complete(&claim, &signed).await?;
                        Ok(DispositionRound::Applied)
                    }
                    Ok(_) => {
                        self.work
                            .reconcile(
                                &claim,
                                DispositionReconcileReason::EvidenceConflict,
                                Some(evidence),
                            )
                            .await?;
                        Ok(DispositionRound::Reconciled)
                    }
                    Err(error) => {
                        tracing::debug!(
                            code = error
                                .api_error()
                                .map_or("consignment_unavailable", |(_, code)| code),
                            "寄售处置证明未被确认"
                        );
                        self.work
                            .reconcile(
                                &claim,
                                DispositionReconcileReason::EvidenceConflict,
                                Some(evidence),
                            )
                            .await?;
                        Ok(DispositionRound::Reconciled)
                    }
                }
            }
        }
    }

    async fn handle_prepare_failure(
        &self,
        claim: &DispositionWorkClaim,
        not_sent: ConsignmentNotSent,
        now: Timestamp,
    ) -> Result<DispositionRound, ConsignmentError> {
        if not_sent.reason.is_retryable() {
            let reason = match not_sent.reason {
                ConsignmentNotSentReason::DeadlineExpired => {
                    DispositionRetryReason::DeadlineExpired
                }
                _ => DispositionRetryReason::TemporaryUnavailable,
            };
            self.work
                .release_for_retry(claim, reason, disposition_retry_at(now, claim.attempt_count))
                .await?;
            return Ok(DispositionRound::Retried);
        }
        let evidence = not_sent.observed_manifest_digest.map(|manifest_digest| {
            DispositionObservationEvidence {
                observation_id: local_evidence_id(claim, manifest_digest),
                manifest_digest,
                session_binding_digest: [0; 32],
            }
        });
        self.work.reconcile(claim, not_sent.reason.reconcile_reason(), evidence).await?;
        Ok(DispositionRound::Reconciled)
    }
}

fn disposition_request(claim: &DispositionWorkClaim) -> ConsignmentDispositionRequest {
    ConsignmentDispositionRequest {
        action: claim.action,
        final_decision_id: claim.final_decision_id,
        work_id: claim.work_id,
        delivery_id: claim.delivery_id,
        attempt_id: claim.attempt_id,
        fence: claim.fence,
        manifest: claim.manifest.clone(),
    }
}

fn evidence_from_prepared(prepared: &dyn PreparedDisposition) -> DispositionObservationEvidence {
    DispositionObservationEvidence {
        observation_id: prepared.observation_id(),
        manifest_digest: prepared.observed_manifest_digest(),
        session_binding_digest: prepared.session_binding_digest(),
    }
}

fn local_evidence_id(claim: &DispositionWorkClaim, manifest_digest: [u8; 32]) -> Uuid {
    let mut bytes = Vec::with_capacity(64);
    bytes.extend_from_slice(b"auction/consignment/disposition-evidence/v1\0");
    bytes.extend_from_slice(claim.work_id.as_bytes());
    bytes.extend_from_slice(&claim.fence.to_be_bytes());
    bytes.extend_from_slice(&manifest_digest);
    Uuid::new_v5(&Uuid::NAMESPACE_URL, &bytes)
}

fn proof_kind(action: DispositionAction) -> MailProofKind {
    match action {
        DispositionAction::Take => MailProofKind::ExactTake,
        DispositionAction::Return => MailProofKind::ExactReturn,
    }
}

fn proof_matches(action: DispositionAction, disposition: ReceiptDisposition) -> bool {
    matches!(
        (action, disposition),
        (DispositionAction::Take, ReceiptDisposition::TakeProven)
            | (DispositionAction::Return, ReceiptDisposition::ReturnProven)
    )
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};
    use std::time::{Duration as StdDuration, Instant};

    use auction_domain::SettlementWorld;
    use auction_domain::consignment::deposit::ReleaseKind;
    use auction_domain::consignment::deposit::{
        EnvelopeItemObservation, EnvelopeObservation, QuarantineReason, SenderIdentity,
    };
    use auction_domain::consignment::{LegacyAuctionOwner, OriginalMailKey};
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::super::frozen_manifest::{
        ConsignmentDispositionRequest, ConsignmentNotSent, ConsignmentNotSentReason,
        ConsignmentSentUnprovenReason, DispositionManifest, DispositionManifestAttachment,
        DispositionObservationEvidence, DispositionReconcileReason, DispositionRetryReason,
        DispositionWorkClaim,
    };
    use super::super::quarantine_recheck::{
        QuarantineMailLocator, QuarantineRecheckClaim, QuarantineRecheckReadError,
        QuarantineRecheckReader, QuarantineRecheckRound, QuarantineRecheckService,
        QuarantineRecheckStore, QuarantineReobservation,
    };
    use super::super::write_transport::{
        ConsignmentDispositionExecution, ConsignmentDispositionProof,
        ConsignmentDispositionTransport, PreparedDisposition,
    };
    use super::{
        DispositionMaintenanceService, DispositionRound, DispositionRoundRequest,
        DispositionWorkStore,
    };
    use crate::ports::consignment::deposit::test_support::FakeDepositStore;
    use crate::ports::consignment::{
        ConsignmentError, ConsignmentFuture, DispositionAction, ObservationReceipt, PostPermit,
        ProofDisposition,
    };
    use crate::services::consignment::DepositCommandService;

    fn run<T>(future: impl Future<Output = T>) -> T {
        let mut future = std::pin::pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        for _ in 0..64 {
            if let Poll::Ready(result) = future.as_mut().poll(&mut context) {
                return result;
            }
        }
        panic!("内存端口未在预期轮数内完成");
    }

    fn now() -> Timestamp {
        Timestamp::UNIX_EPOCH
    }

    fn lease_until() -> Timestamp {
        now() + auction_time::Duration::hours(1)
    }

    fn manifest() -> DispositionManifest {
        DispositionManifest {
            world: SettlementWorld::Main,
            external_mail_id: "100".to_owned(),
            sender: "Sender".to_owned(),
            recipient: "Bot".to_owned(),
            subject: "consign".to_owned(),
            body_digest: [3; 32],
            credits: 0,
            cod: 0,
            received_minute_raw: "2026-11-21 10:00".to_owned(),
            received_at: now(),
            attachments: vec![DispositionManifestAttachment {
                ordinal: 1,
                name: "Test Sword".to_owned(),
                canonical_eid: Some("42".to_owned()),
            }],
            manifest_digest: [4; 32],
        }
    }

    fn claim(work_id: Uuid, fence: u64, claim_token: Uuid) -> DispositionWorkClaim {
        claim_for_action(work_id, fence, claim_token, DispositionAction::Take)
    }

    fn claim_for_action(
        work_id: Uuid,
        fence: u64,
        claim_token: Uuid,
        action: DispositionAction,
    ) -> DispositionWorkClaim {
        DispositionWorkClaim {
            work_id,
            final_decision_id: Uuid::from_u128(10),
            delivery_id: Uuid::from_u128(11),
            action,
            attempt_id: Uuid::from_u128(12),
            fence,
            claim_token,
            attempt_count: 1,
            lease_until: lease_until(),
            manifest: manifest(),
        }
    }

    fn request() -> DispositionRoundRequest {
        request_at(now())
    }

    fn request_at(now: Timestamp) -> DispositionRoundRequest {
        DispositionRoundRequest {
            worker_id: "test-worker".to_owned(),
            now,
            lease_until: now + auction_time::Duration::hours(1),
            deadline: Instant::now() + StdDuration::from_secs(30),
        }
    }

    #[derive(Default)]
    struct WorkCounts {
        completed: u64,
        reconciled: u64,
        observations: u64,
        evidence: u64,
        retried: u64,
    }

    struct LeaseRecord {
        claim_token: Uuid,
        fence: u64,
    }

    struct QueuedWork {
        claim: DispositionWorkClaim,
        retry_at: Option<Timestamp>,
    }

    #[derive(Default)]
    struct WorkState {
        queue: VecDeque<QueuedWork>,
        current: Option<LeaseRecord>,
        completed: u64,
        reconciled: u64,
        observations: u64,
        evidence: u64,
        retried: u64,
    }

    struct TestWorkStore {
        state: Mutex<WorkState>,
    }

    impl TestWorkStore {
        fn new(claims: Vec<DispositionWorkClaim>) -> Self {
            Self {
                state: Mutex::new(WorkState {
                    queue: claims
                        .into_iter()
                        .map(|claim| QueuedWork { claim, retry_at: None })
                        .collect(),
                    ..WorkState::default()
                }),
            }
        }

        fn push(&self, claim: DispositionWorkClaim) {
            self.state
                .lock()
                .expect("工作锁")
                .queue
                .push_back(QueuedWork { claim, retry_at: None });
        }

        fn takeover(&self) {
            let mut state = self.state.lock().expect("工作锁");
            let next_fence =
                state.current.as_ref().map_or(1, |lease| lease.fence.saturating_add(1));
            state.current =
                Some(LeaseRecord { claim_token: Uuid::from_u128(9_999), fence: next_fence });
        }

        fn counts(&self) -> WorkCounts {
            let state = self.state.lock().expect("工作锁");
            WorkCounts {
                completed: state.completed,
                reconciled: state.reconciled,
                observations: state.observations,
                evidence: state.evidence,
                retried: state.retried,
            }
        }

        fn queued(&self) -> usize {
            self.state.lock().expect("工作锁").queue.len()
        }

        fn assert_current(&self, claim: &DispositionWorkClaim) -> Result<(), ConsignmentError> {
            let state = self.state.lock().expect("工作锁");
            match &state.current {
                Some(lease)
                    if lease.claim_token == claim.claim_token && lease.fence == claim.fence =>
                {
                    Ok(())
                }
                _ => Err(ConsignmentError::LeaseLost),
            }
        }
    }

    impl DispositionWorkStore for TestWorkStore {
        fn claim_due(
            &self,
            _worker_id: String,
            now: Timestamp,
            _lease_until: Timestamp,
        ) -> ConsignmentFuture<'_, Option<DispositionWorkClaim>> {
            let claim = {
                let mut state = self.state.lock().expect("工作锁");
                let position = state
                    .queue
                    .iter()
                    .position(|queued| queued.retry_at.is_none_or(|retry_at| retry_at <= now));
                let claim = position
                    .and_then(|position| state.queue.remove(position))
                    .map(|queued| queued.claim);
                if let Some(claim) = &claim {
                    state.current =
                        Some(LeaseRecord { claim_token: claim.claim_token, fence: claim.fence });
                }
                claim
            };
            Box::pin(async move { Ok(claim) })
        }

        fn check_current(&self, claim: &DispositionWorkClaim) -> ConsignmentFuture<'_, ()> {
            let result = self.assert_current(claim);
            Box::pin(async move { result })
        }

        fn release_for_retry(
            &self,
            claim: &DispositionWorkClaim,
            _reason: DispositionRetryReason,
            retry_at: Timestamp,
        ) -> ConsignmentFuture<'_, ()> {
            let result = (|| {
                self.assert_current(claim)?;
                let mut state = self.state.lock().expect("工作锁");
                state.current = None;
                state.retried = state.retried.saturating_add(1);
                state
                    .queue
                    .push_back(QueuedWork { claim: claim.clone(), retry_at: Some(retry_at) });
                Ok(())
            })();
            Box::pin(async move { result })
        }

        fn record_observation(
            &self,
            claim: &DispositionWorkClaim,
            _evidence: DispositionObservationEvidence,
        ) -> ConsignmentFuture<'_, ()> {
            let result = (|| {
                self.assert_current(claim)?;
                let mut state = self.state.lock().expect("工作锁");
                state.observations = state.observations.saturating_add(1);
                state.evidence = state.evidence.saturating_add(1);
                Ok(())
            })();
            Box::pin(async move { result })
        }

        fn reconcile(
            &self,
            claim: &DispositionWorkClaim,
            _reason: DispositionReconcileReason,
            evidence: Option<DispositionObservationEvidence>,
        ) -> ConsignmentFuture<'_, ()> {
            let result = (|| {
                self.assert_current(claim)?;
                let mut state = self.state.lock().expect("工作锁");
                state.current = None;
                state.reconciled = state.reconciled.saturating_add(1);
                if evidence.is_some() {
                    state.evidence = state.evidence.saturating_add(1);
                }
                Ok(())
            })();
            Box::pin(async move { result })
        }

        fn complete(
            &self,
            claim: &DispositionWorkClaim,
            _proof: &super::super::write_transport::SignedConsignmentProof,
        ) -> ConsignmentFuture<'_, ()> {
            let result = (|| {
                self.assert_current(claim)?;
                let mut state = self.state.lock().expect("工作锁");
                state.current = None;
                state.completed = state.completed.saturating_add(1);
                Ok(())
            })();
            Box::pin(async move { result })
        }
    }

    enum PrepareReply {
        Prepared { observation: Uuid, observed_manifest_digest: [u8; 32] },
        NotSent(ConsignmentNotSent),
    }

    enum ExecuteReply {
        NotSent(ConsignmentNotSent),
        Unproven(ConsignmentSentUnprovenReason),
        Proven { action: DispositionAction, signed: bool },
    }

    struct TestPrepared {
        session_binding_digest: [u8; 32],
        observed_manifest_digest: [u8; 32],
        observation_id: Uuid,
    }

    impl PreparedDisposition for TestPrepared {
        fn session_binding_digest(&self) -> [u8; 32] {
            self.session_binding_digest
        }

        fn observed_manifest_digest(&self) -> [u8; 32] {
            self.observed_manifest_digest
        }

        fn observation_id(&self) -> Uuid {
            self.observation_id
        }
    }

    #[derive(Default)]
    struct TestTransport {
        prepare: Mutex<VecDeque<PrepareReply>>,
        execute: Mutex<VecDeque<ExecuteReply>>,
        prepare_calls: AtomicUsize,
        execute_calls: AtomicUsize,
        posts: AtomicUsize,
        takeover_on_prepare: Mutex<Option<Arc<TestWorkStore>>>,
    }

    impl TestTransport {
        fn push_prepare(&self, reply: PrepareReply) {
            self.prepare.lock().expect("准备队列").push_back(reply);
        }

        fn push_execute(&self, reply: ExecuteReply) {
            self.execute.lock().expect("执行队列").push_back(reply);
        }

        fn set_takeover(&self, store: Arc<TestWorkStore>) {
            *self.takeover_on_prepare.lock().expect("接管锁") = Some(store);
        }
    }

    impl ConsignmentDispositionTransport for TestTransport {
        fn prepare(
            &self,
            _request: ConsignmentDispositionRequest,
            _deadline: Instant,
        ) -> ConsignmentFuture<'_, Result<Box<dyn PreparedDisposition>, ConsignmentNotSent>>
        {
            self.prepare_calls.fetch_add(1, Ordering::Relaxed);
            if let Some(store) = self.takeover_on_prepare.lock().expect("接管锁").take() {
                store.takeover();
            }
            let reply = self.prepare.lock().expect("准备队列").pop_front().expect("准备结果");
            let result = match reply {
                PrepareReply::Prepared { observation, observed_manifest_digest } => {
                    Ok(Box::new(TestPrepared {
                        session_binding_digest: [5; 32],
                        observed_manifest_digest,
                        observation_id: observation,
                    }) as Box<dyn PreparedDisposition>)
                }
                PrepareReply::NotSent(not_sent) => Err(not_sent),
            };
            Box::pin(async move { Ok(result) })
        }

        fn execute(
            &self,
            _prepared: Box<dyn PreparedDisposition>,
            _permit: PostPermit,
            _deadline: Instant,
        ) -> ConsignmentFuture<'_, ConsignmentDispositionExecution> {
            self.execute_calls.fetch_add(1, Ordering::Relaxed);
            let reply = self.execute.lock().expect("执行队列").pop_front().expect("执行结果");
            let result = match reply {
                ExecuteReply::NotSent(not_sent) => ConsignmentDispositionExecution::NotSent {
                    reason: not_sent.reason,
                    observed_manifest_digest: not_sent.observed_manifest_digest,
                },
                ExecuteReply::Unproven(reason) => {
                    self.posts.fetch_add(1, Ordering::Relaxed);
                    ConsignmentDispositionExecution::SentUnproven { reason }
                }
                ExecuteReply::Proven { action, signed } => {
                    self.posts.fetch_add(1, Ordering::Relaxed);
                    ConsignmentDispositionExecution::Proven(
                        ConsignmentDispositionProof::from_adapter(
                            action,
                            Uuid::from_u128(31),
                            "test-validator".to_owned(),
                            [9; 32],
                            signed,
                        )
                        .expect("证明形状"),
                    )
                }
            };
            Box::pin(async move { Ok(result) })
        }
    }

    fn prepared(observation: u128) -> PrepareReply {
        PrepareReply::Prepared {
            observation: Uuid::from_u128(observation),
            observed_manifest_digest: manifest().manifest_digest,
        }
    }

    fn setup(
        claims: Vec<DispositionWorkClaim>,
    ) -> (
        Arc<FakeDepositStore>,
        Arc<TestWorkStore>,
        Arc<TestTransport>,
        DispositionMaintenanceService,
    ) {
        let fake = Arc::new(FakeDepositStore::new());
        let commands = Arc::new(DepositCommandService::new(
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
        ));
        let work = Arc::new(TestWorkStore::new(claims));
        let transport = Arc::new(TestTransport::default());
        let service = DispositionMaintenanceService::new(commands, work.clone(), transport.clone());
        (fake, work, transport, service)
    }

    #[test]
    fn first_commit_posts_once_and_applies_only_signed_proof() {
        let (fake, work, transport, service) =
            setup(vec![claim(Uuid::from_u128(1), 7, Uuid::from_u128(2))]);
        transport.push_prepare(prepared(20));
        transport
            .push_execute(ExecuteReply::Proven { action: DispositionAction::Take, signed: true });
        fake.set_next_proof_disposition(ProofDisposition::TakeProven);

        assert_eq!(run(service.run_once(request())).expect("执行"), DispositionRound::Applied);
        assert_eq!(transport.prepare_calls.load(Ordering::Relaxed), 1);
        assert_eq!(transport.execute_calls.load(Ordering::Relaxed), 1);
        assert_eq!(transport.posts.load(Ordering::Relaxed), 1);
        assert_eq!(fake.counters().permits_issued, 1);
        assert_eq!(fake.snapshot().writes.len(), 1);
        assert_eq!(fake.snapshot().ledger.len(), 2);
        let counts = work.counts();
        assert_eq!(counts.completed, 1);
        assert_eq!(counts.reconciled, 0);
        assert_eq!(counts.observations, 0);
        assert_eq!(counts.evidence, 0);
    }

    #[test]
    fn replay_already_committed_never_posts_again() {
        let (fake, work, transport, service) =
            setup(vec![claim(Uuid::from_u128(1), 7, Uuid::from_u128(2))]);
        transport.push_prepare(prepared(20));
        transport
            .push_execute(ExecuteReply::Proven { action: DispositionAction::Take, signed: true });
        fake.set_next_proof_disposition(ProofDisposition::TakeProven);
        assert_eq!(run(service.run_once(request())).expect("首次执行"), DispositionRound::Applied);

        work.push(claim(Uuid::from_u128(1), 8, Uuid::from_u128(3)));
        transport.push_prepare(prepared(21));
        transport
            .push_execute(ExecuteReply::Proven { action: DispositionAction::Take, signed: true });
        assert_eq!(run(service.run_once(request())).expect("重放",), DispositionRound::Reconciled);
        assert_eq!(transport.execute_calls.load(Ordering::Relaxed), 1);
        assert_eq!(transport.posts.load(Ordering::Relaxed), 1);
        assert_eq!(fake.counters().permits_issued, 1);
        assert_eq!(work.counts().completed, 1);
        assert_eq!(work.counts().reconciled, 1);
    }

    #[test]
    fn exact_already_committed_replay_has_zero_new_permit_and_post() {
        let replay = claim(Uuid::from_u128(1), 7, Uuid::from_u128(2));
        let (fake, work, transport, service) = setup(vec![replay.clone()]);
        transport.push_prepare(prepared(20));
        transport
            .push_execute(ExecuteReply::Proven { action: DispositionAction::Take, signed: true });
        fake.set_next_proof_disposition(ProofDisposition::TakeProven);
        assert_eq!(run(service.run_once(request())).expect("首次执行"), DispositionRound::Applied);
        assert_eq!(fake.counters().permits_issued, 1);
        assert_eq!(transport.posts.load(Ordering::Relaxed), 1);
        assert_eq!(transport.execute_calls.load(Ordering::Relaxed), 1);

        work.push(replay);
        transport.push_prepare(prepared(21));
        assert_eq!(
            run(service.run_once(request())).expect("同 fence 精确重放"),
            DispositionRound::Reconciled
        );
        assert_eq!(fake.counters().permits_issued, 1);
        assert_eq!(transport.posts.load(Ordering::Relaxed), 1);
        assert_eq!(transport.execute_calls.load(Ordering::Relaxed), 1);
        assert_eq!(fake.snapshot().writes.len(), 1);
        assert_eq!(work.counts().completed, 1);
        assert_eq!(work.counts().reconciled, 1);
    }

    #[test]
    fn start_outcome_unknown_never_posts_and_replay_is_already_committed() {
        let (fake, work, transport, service) =
            setup(vec![claim(Uuid::from_u128(1), 7, Uuid::from_u128(2))]);
        fake.lose_next_response();
        transport.push_prepare(prepared(20));
        transport
            .push_execute(ExecuteReply::Proven { action: DispositionAction::Take, signed: true });
        assert_eq!(
            run(service.run_once(request())).expect("起写未知",),
            DispositionRound::Reconciled
        );
        assert_eq!(transport.execute_calls.load(Ordering::Relaxed), 0);
        assert_eq!(transport.posts.load(Ordering::Relaxed), 0);
        assert_eq!(fake.counters().permits_issued, 1);

        work.push(claim(Uuid::from_u128(1), 8, Uuid::from_u128(3)));
        transport.push_prepare(prepared(21));
        transport
            .push_execute(ExecuteReply::Proven { action: DispositionAction::Take, signed: true });
        assert_eq!(run(service.run_once(request())).expect("重试"), DispositionRound::Reconciled);
        assert_eq!(transport.execute_calls.load(Ordering::Relaxed), 0);
        assert_eq!(transport.posts.load(Ordering::Relaxed), 0);
        assert_eq!(fake.counters().permits_issued, 1);
        assert_eq!(work.counts().reconciled, 2);
    }

    #[test]
    fn start_store_failure_is_reconciled_without_post() {
        let (fake, work, transport, service) =
            setup(vec![claim(Uuid::from_u128(1), 7, Uuid::from_u128(2))]);
        fake.fail_next_commit();
        transport.push_prepare(prepared(20));
        transport
            .push_execute(ExecuteReply::Proven { action: DispositionAction::Take, signed: true });
        assert_eq!(
            run(service.run_once(request())).expect("起写失败"),
            DispositionRound::Reconciled
        );
        assert!(fake.snapshot().writes.is_empty());
        assert_eq!(fake.counters().permits_issued, 0);
        assert_eq!(transport.execute_calls.load(Ordering::Relaxed), 0);
        assert_eq!(transport.posts.load(Ordering::Relaxed), 0);
        assert_eq!(work.counts().reconciled, 1);
        assert_eq!(work.counts().evidence, 1);
    }

    #[test]
    fn unsigned_proof_only_appends_evidence_and_never_authorizes_position() {
        let (fake, work, transport, service) =
            setup(vec![claim(Uuid::from_u128(1), 7, Uuid::from_u128(2))]);
        transport.push_prepare(prepared(20));
        transport
            .push_execute(ExecuteReply::Proven { action: DispositionAction::Take, signed: false });
        fake.set_next_proof_disposition(ProofDisposition::TakeProven);
        assert_eq!(
            run(service.run_once(request())).expect("未签收证明"),
            DispositionRound::Reconciled
        );
        assert_eq!(transport.posts.load(Ordering::Relaxed), 1);
        assert_eq!(fake.snapshot().ledger.len(), 1);
        assert_eq!(fake.counters().permits_issued, 1);
        assert_eq!(work.counts().completed, 0);
        assert_eq!(work.counts().reconciled, 1);
        assert_eq!(work.counts().observations, 1);
        assert_eq!(work.counts().evidence, 1);
    }

    #[test]
    fn write_unknown_is_reconciled_and_never_resent() {
        let (fake, work, transport, service) =
            setup(vec![claim(Uuid::from_u128(1), 7, Uuid::from_u128(2))]);
        transport.push_prepare(prepared(20));
        transport.push_execute(ExecuteReply::Unproven(ConsignmentSentUnprovenReason::WriteUnknown));
        assert_eq!(
            run(service.run_once(request())).expect("写后未知"),
            DispositionRound::Reconciled
        );
        assert_eq!(transport.posts.load(Ordering::Relaxed), 1);
        assert_eq!(fake.snapshot().ledger.len(), 1);
        assert_eq!(work.counts().completed, 0);
        assert_eq!(work.counts().reconciled, 1);
        assert_eq!(run(service.run_once(request())).expect("不得重发"), DispositionRound::Idle);
        assert_eq!(transport.posts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn manifest_change_before_start_has_zero_permit_and_post() {
        let (fake, work, transport, service) =
            setup(vec![claim(Uuid::from_u128(1), 7, Uuid::from_u128(2))]);
        transport.push_prepare(PrepareReply::NotSent(ConsignmentNotSent {
            reason: ConsignmentNotSentReason::ManifestChanged,
            observed_manifest_digest: Some([8; 32]),
        }));
        assert_eq!(
            run(service.run_once(request())).expect("清单变化"),
            DispositionRound::Reconciled
        );
        assert_eq!(fake.snapshot().ledger.len(), 0);
        assert_eq!(fake.counters().permits_issued, 0);
        assert_eq!(transport.execute_calls.load(Ordering::Relaxed), 0);
        assert_eq!(transport.posts.load(Ordering::Relaxed), 0);
        assert_eq!(work.counts().reconciled, 1);
        assert_eq!(work.counts().evidence, 1);
    }

    #[test]
    fn prepared_manifest_mismatch_has_zero_permit_and_post() {
        let (fake, work, transport, service) =
            setup(vec![claim(Uuid::from_u128(1), 7, Uuid::from_u128(2))]);
        transport.push_prepare(PrepareReply::Prepared {
            observation: Uuid::from_u128(20),
            observed_manifest_digest: [8; 32],
        });
        assert_eq!(
            run(service.run_once(request())).expect("准备摘要变化"),
            DispositionRound::Reconciled
        );
        assert_eq!(transport.prepare_calls.load(Ordering::Relaxed), 1);
        assert_eq!(transport.execute_calls.load(Ordering::Relaxed), 0);
        assert_eq!(transport.posts.load(Ordering::Relaxed), 0);
        assert_eq!(fake.counters().permits_issued, 0);
        assert!(fake.snapshot().writes.is_empty());
        assert!(fake.snapshot().ledger.is_empty());
        assert_eq!(work.counts().reconciled, 1);
        assert_eq!(work.counts().evidence, 1);
    }

    #[test]
    fn manifest_change_after_start_has_one_permit_but_zero_post() {
        let (fake, work, transport, service) =
            setup(vec![claim(Uuid::from_u128(1), 7, Uuid::from_u128(2))]);
        transport.push_prepare(prepared(20));
        transport.push_execute(ExecuteReply::NotSent(ConsignmentNotSent {
            reason: ConsignmentNotSentReason::ManifestChanged,
            observed_manifest_digest: Some([8; 32]),
        }));
        assert_eq!(
            run(service.run_once(request())).expect("写前清单变化"),
            DispositionRound::Reconciled
        );
        assert_eq!(transport.execute_calls.load(Ordering::Relaxed), 1);
        assert_eq!(transport.posts.load(Ordering::Relaxed), 0);
        assert_eq!(fake.counters().permits_issued, 1);
        assert_eq!(fake.snapshot().writes.len(), 1);
        assert_eq!(work.counts().reconciled, 1);
        assert_eq!(work.counts().evidence, 1);
    }

    #[test]
    fn lease_takeover_before_start_write_leaves_old_holder_without_post() {
        let (fake, work, transport, service) =
            setup(vec![claim(Uuid::from_u128(1), 7, Uuid::from_u128(2))]);
        transport.set_takeover(work.clone());
        transport.push_prepare(prepared(20));
        transport
            .push_execute(ExecuteReply::Proven { action: DispositionAction::Take, signed: true });
        assert_eq!(
            run(service.run_once(request())).expect("租约接管"),
            DispositionRound::LeaseLost
        );
        assert!(fake.snapshot().ledger.is_empty());
        assert_eq!(fake.counters().permits_issued, 0);
        assert_eq!(transport.execute_calls.load(Ordering::Relaxed), 0);
        assert_eq!(transport.posts.load(Ordering::Relaxed), 0);
        assert_eq!(work.counts().completed, 0);
    }

    #[test]
    fn claim_expired_at_lease_boundary_is_lease_lost_without_any_write() {
        let mut expired = claim(Uuid::from_u128(1), 7, Uuid::from_u128(2));
        expired.lease_until = now();
        let (fake, _work, transport, service) = setup(vec![expired]);
        assert_eq!(
            run(service.run_once(request_at(now()))).expect("租约等号即过期"),
            DispositionRound::LeaseLost
        );
        assert_eq!(transport.prepare_calls.load(Ordering::Relaxed), 0);
        assert_eq!(transport.execute_calls.load(Ordering::Relaxed), 0);
        assert_eq!(transport.posts.load(Ordering::Relaxed), 0);
        assert_eq!(fake.counters().permits_issued, 0);
        assert!(fake.snapshot().writes.is_empty());
        assert!(fake.snapshot().ledger.is_empty());
    }

    #[test]
    fn temporary_prepare_failure_releases_the_work_for_retry() {
        let (fake, work, transport, service) =
            setup(vec![claim(Uuid::from_u128(1), 7, Uuid::from_u128(2))]);
        transport.push_prepare(PrepareReply::NotSent(ConsignmentNotSent {
            reason: ConsignmentNotSentReason::TemporaryUnavailable,
            observed_manifest_digest: None,
        }));
        assert_eq!(run(service.run_once(request())).expect("临时失败"), DispositionRound::Retried);
        assert!(fake.snapshot().ledger.is_empty());
        assert_eq!(fake.counters().permits_issued, 0);
        assert_eq!(work.counts().retried, 1);
        assert_eq!(work.queued(), 1);
    }

    #[test]
    fn retried_work_is_not_claimable_before_retry_at_and_is_due_at_the_boundary() {
        let (fake, work, transport, service) =
            setup(vec![claim(Uuid::from_u128(1), 7, Uuid::from_u128(2))]);
        transport.push_prepare(PrepareReply::NotSent(ConsignmentNotSent {
            reason: ConsignmentNotSentReason::TemporaryUnavailable,
            observed_manifest_digest: None,
        }));
        assert_eq!(run(service.run_once(request())).expect("临时失败"), DispositionRound::Retried);
        assert_eq!(transport.prepare_calls.load(Ordering::Relaxed), 1);

        assert_eq!(
            run(service.run_once(request_at(now() + auction_time::Duration::seconds(4))))
                .expect("未到重试时刻"),
            DispositionRound::Idle
        );
        assert_eq!(transport.prepare_calls.load(Ordering::Relaxed), 1);

        transport.push_prepare(prepared(21));
        transport
            .push_execute(ExecuteReply::Proven { action: DispositionAction::Take, signed: true });
        fake.set_next_proof_disposition(ProofDisposition::TakeProven);
        assert_eq!(
            run(service.run_once(request_at(now() + auction_time::Duration::seconds(5))))
                .expect("重试到期"),
            DispositionRound::Applied
        );
        assert_eq!(transport.prepare_calls.load(Ordering::Relaxed), 2);
        assert_eq!(work.counts().retried, 1);
        assert_eq!(work.counts().completed, 1);
    }

    #[test]
    fn return_action_uses_the_return_proof_and_completes() {
        let (fake, work, transport, service) = setup(vec![claim_for_action(
            Uuid::from_u128(1),
            7,
            Uuid::from_u128(2),
            DispositionAction::Return,
        )]);
        transport.push_prepare(prepared(20));
        transport
            .push_execute(ExecuteReply::Proven { action: DispositionAction::Return, signed: true });
        fake.set_next_proof_disposition(ProofDisposition::ReturnProven);
        assert_eq!(run(service.run_once(request())).expect("退回"), DispositionRound::Applied);
        assert_eq!(work.counts().completed, 1);
        assert_eq!(fake.snapshot().ledger.len(), 2);
    }

    fn quarantine_key() -> OriginalMailKey {
        OriginalMailKey::new("42", SettlementWorld::Main, "100").expect("原信键")
    }

    fn observation() -> EnvelopeObservation {
        EnvelopeObservation {
            observation_id: Uuid::from_u128(70),
            session_binding_digest: [1; 32],
            manifest_digest: [3; 32],
            parser_version: "test-parser".to_owned(),
            subject_is_consign: true,
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
            system_rts: false,
            legacy_auction_owner: None,
            items: Vec::<EnvelopeItemObservation>::new(),
        }
    }

    fn quarantine_claim(
        quarantine_id: Uuid,
        version: u64,
        claim_token: Uuid,
        fence: u64,
    ) -> QuarantineRecheckClaim {
        QuarantineRecheckClaim {
            work_id: Uuid::from_u128(73),
            quarantine_id,
            quarantine_version: version,
            mail_key: quarantine_key(),
            locator: QuarantineMailLocator {
                world: SettlementWorld::Main,
                external_mail_id: "100".to_owned(),
                sender: "Sender".to_owned(),
                subject: "consign".to_owned(),
                received_minute_raw: "2026-11-21 10:00".to_owned(),
                received_at: now(),
            },
            previous_observation_id: Uuid::from_u128(87),
            previous_binding: None,
            claim_token,
            attempt_count: 1,
            fence,
            lease_until: lease_until(),
        }
    }

    struct QueuedQuarantine {
        claim: QuarantineRecheckClaim,
        retry_at: Option<Timestamp>,
    }

    struct QuarantineWorkState {
        queue: VecDeque<QueuedQuarantine>,
        current: Option<LeaseRecord>,
        completed: u64,
        reconciled: u64,
        retried: u64,
    }

    struct TestQuarantineStore {
        state: Mutex<QuarantineWorkState>,
    }

    impl TestQuarantineStore {
        fn new(claims: Vec<QuarantineRecheckClaim>) -> Self {
            Self {
                state: Mutex::new(QuarantineWorkState {
                    queue: claims
                        .into_iter()
                        .map(|claim| QueuedQuarantine { claim, retry_at: None })
                        .collect(),
                    current: None,
                    completed: 0,
                    reconciled: 0,
                    retried: 0,
                }),
            }
        }

        fn push(&self, claim: QuarantineRecheckClaim) {
            self.state
                .lock()
                .expect("重检锁")
                .queue
                .push_back(QueuedQuarantine { claim, retry_at: None });
        }

        fn takeover(&self) {
            let mut state = self.state.lock().expect("重检锁");
            let next_fence =
                state.current.as_ref().map_or(1, |lease| lease.fence.saturating_add(1));
            state.current =
                Some(LeaseRecord { claim_token: Uuid::from_u128(9_998), fence: next_fence });
        }

        fn assert_current(&self, claim: &QuarantineRecheckClaim) -> Result<(), ConsignmentError> {
            let state = self.state.lock().expect("重检锁");
            match &state.current {
                Some(lease)
                    if lease.claim_token == claim.claim_token && lease.fence == claim.fence =>
                {
                    Ok(())
                }
                _ => Err(ConsignmentError::LeaseLost),
            }
        }

        fn counts(&self) -> (u64, u64, u64) {
            let state = self.state.lock().expect("重检锁");
            (state.completed, state.reconciled, state.retried)
        }
    }

    impl QuarantineRecheckStore for TestQuarantineStore {
        fn claim_due(
            &self,
            _worker_id: String,
            now: Timestamp,
            _lease_until: Timestamp,
        ) -> ConsignmentFuture<'_, Option<QuarantineRecheckClaim>> {
            let claim = {
                let mut state = self.state.lock().expect("重检锁");
                let position = state
                    .queue
                    .iter()
                    .position(|queued| queued.retry_at.is_none_or(|retry_at| retry_at <= now));
                let claim = position
                    .and_then(|position| state.queue.remove(position))
                    .map(|queued| queued.claim);
                if let Some(claim) = &claim {
                    state.current =
                        Some(LeaseRecord { claim_token: claim.claim_token, fence: claim.fence });
                }
                claim
            };
            Box::pin(async move { Ok(claim) })
        }

        fn check_current(&self, claim: &QuarantineRecheckClaim) -> ConsignmentFuture<'_, ()> {
            let result = self.assert_current(claim);
            Box::pin(async move { result })
        }

        fn release_for_retry(
            &self,
            claim: &QuarantineRecheckClaim,
            _reason: DispositionRetryReason,
            retry_at: Timestamp,
        ) -> ConsignmentFuture<'_, ()> {
            let result = (|| {
                self.assert_current(claim)?;
                let mut state = self.state.lock().expect("重检锁");
                state.current = None;
                state.retried = state.retried.saturating_add(1);
                state
                    .queue
                    .push_back(QueuedQuarantine { claim: claim.clone(), retry_at: Some(retry_at) });
                Ok(())
            })();
            Box::pin(async move { result })
        }

        fn complete(
            &self,
            claim: &QuarantineRecheckClaim,
            _receipt: &ObservationReceipt,
        ) -> ConsignmentFuture<'_, ()> {
            let result = (|| {
                self.assert_current(claim)?;
                let mut state = self.state.lock().expect("重检锁");
                state.current = None;
                state.completed = state.completed.saturating_add(1);
                Ok(())
            })();
            Box::pin(async move { result })
        }

        fn reconcile(
            &self,
            claim: &QuarantineRecheckClaim,
            _reason: DispositionReconcileReason,
        ) -> ConsignmentFuture<'_, ()> {
            let result = (|| {
                self.assert_current(claim)?;
                let mut state = self.state.lock().expect("重检锁");
                state.current = None;
                state.reconciled = state.reconciled.saturating_add(1);
                Ok(())
            })();
            Box::pin(async move { result })
        }
    }

    struct TestReader {
        replies: Mutex<VecDeque<Result<QuarantineReobservation, QuarantineRecheckReadError>>>,
        calls: AtomicUsize,
        takeover_on_read: Mutex<Option<Arc<TestQuarantineStore>>>,
    }

    impl TestReader {
        fn new(
            replies: impl IntoIterator<
                Item = Result<QuarantineReobservation, QuarantineRecheckReadError>,
            >,
        ) -> Self {
            Self {
                replies: Mutex::new(replies.into_iter().collect()),
                calls: AtomicUsize::default(),
                takeover_on_read: Mutex::new(None),
            }
        }

        fn set_takeover(&self, store: Arc<TestQuarantineStore>) {
            *self.takeover_on_read.lock().expect("重检接管锁") = Some(store);
        }
    }

    impl QuarantineRecheckReader for TestReader {
        fn reread(
            &self,
            _claim: &QuarantineRecheckClaim,
            _deadline: Instant,
        ) -> ConsignmentFuture<'_, Result<QuarantineReobservation, QuarantineRecheckReadError>>
        {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if let Some(store) = self.takeover_on_read.lock().expect("重检接管锁").take() {
                store.takeover();
            }
            let reply = self.replies.lock().expect("重检结果").pop_front().expect("重检结果");
            Box::pin(async move { Ok(reply) })
        }
    }

    fn reobservation(remote: &str, digest: [u8; 32]) -> QuarantineReobservation {
        let mut observed = observation();
        observed.manifest_digest = digest;
        QuarantineReobservation {
            remote_observation_id: remote.to_owned(),
            observed_at: now(),
            observation: observed,
            evidence_ref: None,
        }
    }

    fn quarantine_setup(
        replies: impl IntoIterator<Item = Result<QuarantineReobservation, QuarantineRecheckReadError>>,
    ) -> (
        Arc<FakeDepositStore>,
        Arc<TestQuarantineStore>,
        Arc<TestReader>,
        QuarantineRecheckService,
        Uuid,
    ) {
        let fake = Arc::new(FakeDepositStore::new());
        let quarantine_id = fake.seed_quarantine(
            quarantine_key(),
            QuarantineReason::TakeUnproven,
            Uuid::from_u128(71),
        );
        let commands = Arc::new(DepositCommandService::new(
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
        ));
        let work = Arc::new(TestQuarantineStore::new(Vec::new()));
        let reader = Arc::new(TestReader::new(replies));
        let service = QuarantineRecheckService::new(commands, work.clone(), reader.clone());
        (fake, work, reader, service, quarantine_id)
    }

    #[test]
    fn quarantine_recheck_only_appends_observation_and_never_touches_assets() {
        let (fake, work, _reader, service, quarantine_id) =
            quarantine_setup([Ok(reobservation("r1", [6; 32]))]);
        work.push(quarantine_claim(quarantine_id, 1, Uuid::from_u128(76), 1));
        let result = run(service.run_once(request())).expect("重检");
        assert!(matches!(result, QuarantineRecheckRound::Appended { quarantine_version: 2, .. }));
        let snapshot = fake.snapshot();
        assert_eq!(snapshot.quarantines.get(&quarantine_id).expect("隔离").version, 2);
        assert_eq!(snapshot.quarantines.get(&quarantine_id).expect("隔离").observations.len(), 2);
        assert_eq!(snapshot.ledger.len(), 1);
        assert!(snapshot.writes.is_empty());
        assert!(snapshot.terminals.is_empty());
        assert_eq!(fake.counters().permits_issued, 0);
        assert_eq!(fake.counters().posts_recorded, 0);
        assert_eq!(work.counts(), (1, 0, 0));
    }

    #[test]
    fn same_observation_replay_does_not_increment_quarantine_version() {
        let (fake, work, _reader, service, quarantine_id) = quarantine_setup([
            Ok(reobservation("same", [6; 32])),
            Ok(reobservation("same", [6; 32])),
        ]);
        work.push(quarantine_claim(quarantine_id, 1, Uuid::from_u128(80), 1));
        run(service.run_once(request())).expect("首次观察");
        work.push(quarantine_claim(quarantine_id, 2, Uuid::from_u128(81), 2));
        run(service.run_once(request())).expect("同观察重放");
        let snapshot = fake.snapshot();
        assert_eq!(snapshot.quarantines.get(&quarantine_id).expect("隔离").version, 2);
        assert_eq!(snapshot.quarantines.get(&quarantine_id).expect("隔离").observations.len(), 2);
        assert_eq!(snapshot.ledger.len(), 1);
    }

    #[test]
    fn conflicting_observation_identity_leaves_version_unchanged() {
        let (fake, work, _reader, service, quarantine_id) = quarantine_setup([
            Ok(reobservation("same", [6; 32])),
            Ok(reobservation("same", [7; 32])),
        ]);
        work.push(quarantine_claim(quarantine_id, 1, Uuid::from_u128(82), 1));
        run(service.run_once(request())).expect("首次观察");
        work.push(quarantine_claim(quarantine_id, 2, Uuid::from_u128(83), 2));
        assert_eq!(
            run(service.run_once(request())).expect("冲突观察"),
            QuarantineRecheckRound::Reconciled
        );
        let snapshot = fake.snapshot();
        assert_eq!(snapshot.quarantines.get(&quarantine_id).expect("隔离").version, 2);
        assert_eq!(snapshot.quarantines.get(&quarantine_id).expect("隔离").observations.len(), 2);
        assert_eq!(work.counts(), (1, 1, 0));
    }

    #[test]
    fn quarantine_recheck_lease_takeover_does_not_append_after_read() {
        // append 端口没有 fence 参数：4C 必须在 append 事务内核对重检工作的 fence，不一致
        // 就拒绝。若检查与追加之间恰好过租，旧持有者最多多写一条有效观察、隔离版本加一、
        // 不产生任何资产动作；这里钉住应用层在读取后、追加前仍会重新核对 claim/fence。
        let (fake, work, reader, service, quarantine_id) =
            quarantine_setup([Ok(reobservation("r1", [6; 32]))]);
        reader.set_takeover(work.clone());
        work.push(quarantine_claim(quarantine_id, 1, Uuid::from_u128(84), 1));
        assert_eq!(
            run(service.run_once(request())).expect("重检接管"),
            QuarantineRecheckRound::LeaseLost
        );
        let snapshot = fake.snapshot();
        assert_eq!(snapshot.quarantines.get(&quarantine_id).expect("隔离").version, 1);
        assert_eq!(snapshot.quarantines.get(&quarantine_id).expect("隔离").observations.len(), 1);
        assert!(snapshot.ledger.is_empty());
    }

    #[test]
    fn quarantine_recheck_temporary_read_releases_for_retry() {
        let (fake, work, _reader, service, quarantine_id) =
            quarantine_setup([Err(QuarantineRecheckReadError::TemporaryUnavailable)]);
        work.push(quarantine_claim(quarantine_id, 1, Uuid::from_u128(85), 1));
        assert_eq!(
            run(service.run_once(request())).expect("临时读取失败"),
            QuarantineRecheckRound::Retried
        );
        assert!(fake.snapshot().ledger.is_empty());
        assert_eq!(work.counts(), (0, 0, 1));
    }

    #[test]
    fn quarantine_retry_is_not_claimable_before_retry_at_and_is_due_at_the_boundary() {
        let (fake, work, reader, service, quarantine_id) = quarantine_setup([
            Err(QuarantineRecheckReadError::TemporaryUnavailable),
            Ok(reobservation("r1", [6; 32])),
        ]);
        work.push(quarantine_claim(quarantine_id, 1, Uuid::from_u128(85), 1));
        assert_eq!(
            run(service.run_once(request())).expect("临时读取失败"),
            QuarantineRecheckRound::Retried
        );
        assert_eq!(reader.calls.load(Ordering::Relaxed), 1);

        assert_eq!(
            run(service.run_once(request_at(now() + auction_time::Duration::seconds(4))))
                .expect("未到重试时刻"),
            QuarantineRecheckRound::Idle
        );
        assert_eq!(reader.calls.load(Ordering::Relaxed), 1);

        assert!(matches!(
            run(service.run_once(request_at(now() + auction_time::Duration::seconds(5))))
                .expect("重试到期"),
            QuarantineRecheckRound::Appended { .. }
        ));
        assert_eq!(reader.calls.load(Ordering::Relaxed), 2);
        assert!(fake.snapshot().writes.is_empty());
        assert_eq!(fake.counters().permits_issued, 0);
        assert_eq!(fake.counters().posts_recorded, 0);
    }

    #[test]
    fn reader_owner_hint_hands_off_without_asset_work() {
        let mut observed = reobservation("legacy-owner", [6; 32]);
        observed.observation.legacy_auction_owner = Some(LegacyAuctionOwner::BuyerRefund);
        let (fake, work, _reader, service, quarantine_id) = quarantine_setup([Ok(observed)]);
        work.push(quarantine_claim(quarantine_id, 1, Uuid::from_u128(86), 1));
        assert!(matches!(
            run(service.run_once(request())).expect("追加旧 owner 观察并交接"),
            QuarantineRecheckRound::Appended { quarantine_version: 2, .. }
        ));
        let snapshot = fake.snapshot();
        let quarantine = snapshot.quarantines.get(&quarantine_id).expect("隔离");
        assert_eq!(quarantine.version, 2);
        assert_eq!(quarantine.observations.len(), 2);
        assert_eq!(quarantine.observations.last().expect("新观察").digest, [6; 32]);
        assert_eq!(quarantine.reason, QuarantineReason::TakeUnproven);
        assert!(!quarantine.active);
        assert_eq!(quarantine.owner, Some(ReleaseKind::AuctionReturnCase));
        assert!(quarantine.release_receipt.is_some());
        assert_eq!(snapshot.settlement_owners, 1);
        assert_eq!(snapshot.recovery_owners, 0);
        assert!(snapshot.writes.is_empty());
        assert!(snapshot.terminals.is_empty());
        assert_eq!(fake.counters().permits_issued, 0);
        assert_eq!(fake.counters().posts_recorded, 0);
        assert_eq!(work.counts(), (1, 0, 0));
    }
}
