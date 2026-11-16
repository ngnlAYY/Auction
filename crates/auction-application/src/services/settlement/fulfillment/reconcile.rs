//! 结算履约 `enqueued` 阶段：读取 MoogleMail Sent 证据并推进对账终态。

use chrono::{DateTime, Utc};

use super::authorization::{AuthorizationResult, authorize_enqueued_recipient};
use super::evidence::validate_sent_evidence;
use super::intent::build_delivery_intent;
use super::proof::proof_digest;
use super::schedule::{
    collection_poll_delay, collection_wait_interval, evidence_poll_delay,
    evidence_read_failure_delay, widen_for_collection_wait,
};
use super::snapshot::delivery_snapshot_matches;
use super::validation::{claim_context, repository_error, valid_external_mail_id, validate_claim};
use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, SafeErrorKind, record_error,
};
use crate::ports::settlement::{
    SettlementFulfillmentClaim, SettlementFulfillmentDeliveryStatus,
    SettlementFulfillmentOperation, SettlementFulfillmentProof,
    SettlementFulfillmentReconcileResult, SettlementFulfillmentReconcileStatus,
    SettlementFulfillmentState,
};
use crate::services::settlement::SettlementFulfillmentService;

impl SettlementFulfillmentService {
    #[tracing::instrument(
        skip_all,
        fields(
            obligation_id = %claim.obligation_id,
            delivery_id = ?claim.delivery_id,
        )
    )]
    pub(super) async fn reconcile_now(
        &self,
        claim: SettlementFulfillmentClaim,
        observed_at: DateTime<Utc>,
        retry_at: DateTime<Utc>,
    ) -> Result<SettlementFulfillmentReconcileResult, ApplicationError> {
        validate_claim(&claim)?;
        if claim.state != SettlementFulfillmentState::Enqueued
            || observed_at.timestamp_nanos_opt().is_none()
            || observed_at < claim.claimed_at
            || retry_at <= observed_at
        {
            return Err(repository_error("当前履约阶段不允许读取发送回执"));
        }
        let retry_delay = retry_at - observed_at;
        // 证据等待按该义务已持久化的延期次数指数退避，最多 10 分钟；
        // 付款截止前后恢复基础间隔。远端 Sent 读取失败同样退避；本地依赖与身份错误仍按
        // 配置间隔重试，不进入退避。
        let evidence_delay = evidence_poll_delay(
            retry_delay,
            claim.retry_count,
            claim.payment_deadline_at,
            observed_at,
        );
        let Some(delivery_id) = claim.delivery_id else {
            return Err(repository_error("发送回执缺少 delivery identity"));
        };
        if matches!(
            claim.kind,
            auction_domain::SettlementObligationKind::WinnerEquipmentCod
                | auction_domain::SettlementObligationKind::WinnerPaymentCarrierCod
        ) && let Some((store, _)) = &self.internal
        {
            let command = crate::ports::settlement::SettlementInternalCommand::new(
                claim.clone(),
                None,
                true,
            )?;
            if store.replay_internal(command).await? {
                return Ok(SettlementFulfillmentReconcileResult {
                    obligation_id: claim.obligation_id,
                    delivery_id,
                    status: SettlementFulfillmentReconcileStatus::Satisfied,
                });
            }
        }
        if claim.operation == SettlementFulfillmentOperation::Return {
            return self
                .manual_reconcile_now(claim, delivery_id, "legacy_return_requires_review")
                .await;
        }
        let claim = match authorize_enqueued_recipient(
            self.store.as_ref(),
            self.recipient_reader.as_ref(),
            claim,
            retry_delay,
        )
        .await?
        {
            AuthorizationResult::Authorized(claim) => claim,
            AuthorizationResult::Deferred { obligation_id, .. } => {
                return Ok(SettlementFulfillmentReconcileResult {
                    obligation_id,
                    delivery_id,
                    status: SettlementFulfillmentReconcileStatus::Deferred,
                });
            }
            AuthorizationResult::ManualReconcile { obligation_id, .. } => {
                return Ok(SettlementFulfillmentReconcileResult {
                    obligation_id,
                    delivery_id,
                    status: SettlementFulfillmentReconcileStatus::ManualReconcile,
                });
            }
        };
        let intent = match build_delivery_intent(&claim) {
            Ok(intent) => intent,
            Err(_) => {
                return self
                    .manual_reconcile_now(claim, delivery_id, "invalid_fulfillment_fact")
                    .await;
            }
        };
        if !delivery_snapshot_matches(&claim, &intent) {
            return self.manual_reconcile_now(claim, delivery_id, "delivery_conflict").await;
        }
        let (delivery_status, external_reference, external_reference_proven, sent_at) = {
            let snapshot = claim
                .delivery_snapshot
                .as_ref()
                .ok_or_else(|| repository_error("发送回执缺少 delivery 快照"))?;
            (
                snapshot.status,
                snapshot.external_reference.clone(),
                snapshot.external_reference_proven,
                snapshot.completed_at,
            )
        };
        match delivery_status {
            SettlementFulfillmentDeliveryStatus::Pending
            | SettlementFulfillmentDeliveryStatus::Claimed
            | SettlementFulfillmentDeliveryStatus::Sending => {
                // 发送与履约是独立循环；先释放本阶段 claim，再以同一事务写入追踪时间。
                let poll_delay = collection_poll_delay(
                    self.collection_next_poll(&claim, observed_at),
                    observed_at,
                    evidence_delay,
                    claim.payment_deadline_at,
                );
                record_error(
                    DiagnosticLevel::Info,
                    SafeErrorKind::EVIDENCE_NOT_OBSERVED,
                    "fulfillment",
                    "wait_delivery_terminal",
                    DiagnosticContext {
                        defer_reason: Some("evidence_not_observed"),
                        ..claim_context(&claim)
                    },
                );
                return self
                    .defer_now(claim, delivery_id, poll_delay, "evidence_not_observed")
                    .await;
            }
            SettlementFulfillmentDeliveryStatus::Failed => {
                return self.failed_now(claim, delivery_id, "delivery_failed").await;
            }
            SettlementFulfillmentDeliveryStatus::ManualReconcile => {
                return self
                    .manual_reconcile_now(claim, delivery_id, "fulfillment_outcome_unknown")
                    .await;
            }
            SettlementFulfillmentDeliveryStatus::Succeeded => {}
            SettlementFulfillmentDeliveryStatus::Prepared => {
                return self.manual_reconcile_now(claim, delivery_id, "delivery_conflict").await;
            }
        }
        let Some(external_reference) = external_reference.as_deref() else {
            return self
                .manual_reconcile_now(claim, delivery_id, "fulfillment_outcome_unknown")
                .await;
        };
        let Some(remote_id) = fulfillment_reference_suffix(
            external_reference,
            claim.world,
            SettlementFulfillmentOperation::Send,
        ) else {
            return self
                .manual_reconcile_now(claim, delivery_id, "fulfillment_outcome_unknown")
                .await;
        };
        if !external_reference_proven
            || !valid_external_mail_id(remote_id)
            || remote_id == delivery_id.to_string()
        {
            return self
                .manual_reconcile_now(claim, delivery_id, "fulfillment_outcome_unknown")
                .await;
        }

        // 货款、退款、寄回与交付已寄出、只等收件人领取：按寄出时长放宽下一次 Sent 读取（分档只
        // 给出最小间隔，只放宽、不收紧）；COD 与载体付款等的是买家付款，不参与分档。本地依赖
        // 错误的快速重试保持不变；领取证明与状态判定不受影响。
        let collection_wait =
            collection_wait_interval(claim.kind, sent_at, observed_at, claim.payment_deadline_at);
        let awaiting_delay = widen_for_collection_wait(
            evidence_delay,
            collection_wait,
            observed_at,
            claim.payment_deadline_at,
        );
        let sent = match self.evidence_reader.read_sent(claim.world, remote_id.to_owned()).await {
            Ok(sent) => sent,
            Err(ApplicationError::FulfillmentEvidenceNotObserved) => {
                record_error(
                    DiagnosticLevel::Info,
                    SafeErrorKind::EVIDENCE_NOT_OBSERVED,
                    "fulfillment",
                    "wait_sent_receipt",
                    DiagnosticContext {
                        defer_reason: Some("evidence_not_observed"),
                        ..claim_context(&claim)
                    },
                );
                return self
                    .defer_now(claim, delivery_id, awaiting_delay, "evidence_not_observed")
                    .await;
            }
            Err(ApplicationError::FulfillmentDependencyMismatch) => {
                return self.manual_reconcile_now(claim, delivery_id, "evidence_mismatch").await;
            }
            Err(error) => {
                // 账号忙是正常退避；其他证据读取错误保留 WARN 供排查。
                let level = match error.diagnostic_kind() {
                    SafeErrorKind::REMOTE_BUSY | SafeErrorKind::REMOTE_ACCOUNT_BUSY => {
                        DiagnosticLevel::Info
                    }
                    _ => DiagnosticLevel::Warn,
                };
                record_error(
                    level,
                    error.diagnostic_kind(),
                    "fulfillment",
                    "read_sent_receipt",
                    claim_context(&claim),
                );

                let delay = evidence_read_failure_delay(&error, retry_delay, awaiting_delay);
                return self.defer_now(claim, delivery_id, delay, "evidence_unavailable").await;
            }
        };
        let checked_at = self.clock.now();
        if sent.observed_at > checked_at
            || validate_sent_evidence(&claim, remote_id, &sent).is_err()
        {
            return self.manual_reconcile_now(claim, delivery_id, "evidence_mismatch").await;
        }
        if !sent.collected {
            record_error(
                DiagnosticLevel::Info,
                SafeErrorKind::EVIDENCE_NOT_OBSERVED,
                "fulfillment",
                "wait_asset_collection",
                DiagnosticContext {
                    defer_reason: Some("evidence_not_observed"),
                    ..claim_context(&claim)
                },
            );
            // 买家支付 COD、卖家领取货款都只能等对方在游戏内操作，统一对到下一个追踪窗口；
            // 货款等非付款义务寄出满 24 小时后再按寄出时长分档放宽，COD 与载体付款保持窗口。
            let poll_delay = widen_for_collection_wait(
                collection_poll_delay(
                    self.collection_next_poll(&claim, observed_at),
                    observed_at,
                    evidence_delay,
                    claim.payment_deadline_at,
                ),
                collection_wait,
                observed_at,
                claim.payment_deadline_at,
            );
            return self.defer_now(claim, delivery_id, poll_delay, "evidence_not_observed").await;
        }
        let proof = SettlementFulfillmentProof {
            kind: "mooglemail_sent_collected".to_owned(),
            reference: remote_id.to_owned(),
            digest: proof_digest(delivery_id, &sent)?,
            observed_at: sent.observed_at,
        };
        if matches!(
            claim.kind,
            auction_domain::SettlementObligationKind::WinnerEquipmentCod
                | auction_domain::SettlementObligationKind::WinnerPaymentCarrierCod
        ) {
            return self.reconcile_collected(claim, proof, retry_delay).await;
        }
        self.store.satisfy(claim, proof).await
    }
}

fn fulfillment_reference_suffix(
    reference: &str,
    world: auction_domain::SettlementWorld,
    operation: SettlementFulfillmentOperation,
) -> Option<&str> {
    let prefix = format!("mooglemail:{}:{}:", world.as_str(), operation.as_str());
    reference.strip_prefix(&prefix).filter(|value| !value.is_empty() && *value == value.trim())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use auction_domain::{
        AuctionId, CommandId, EquipmentId, LotId, SettlementId, SettlementObligationKind,
        SettlementWorld,
    };
    use chrono::{DateTime, Duration, TimeZone, Utc};
    use uuid::Uuid;

    use self::super::super::intent::{
        build_canonical_delivery, build_delivery_intent, stable_delivery_id,
        stable_prepare_command_id,
    };
    use crate::errors::ApplicationError;
    use crate::errors::diagnostics::SafeErrorKind;
    use crate::ports::core::{ApplicationFuture, Clock, RepositoryError, RequestHash};
    use crate::ports::settlement::{
        InternalEvidenceError, InternalEvidenceFuture, InternalEvidenceRequest,
        PreparedSettlementInternalEvidence, SettlementDeliveryPreparation,
        SettlementFulfillmentActivationResult, SettlementFulfillmentBalanceEvidence,
        SettlementFulfillmentClaim, SettlementFulfillmentCustodyReader,
        SettlementFulfillmentDeliverySnapshot, SettlementFulfillmentDeliveryStatus,
        SettlementFulfillmentEvidenceReader, SettlementFulfillmentOperation,
        SettlementFulfillmentProof, SettlementFulfillmentRecipientReader,
        SettlementFulfillmentRecipientSnapshot, SettlementFulfillmentReconcileResult,
        SettlementFulfillmentReconcileStatus, SettlementFulfillmentResult,
        SettlementFulfillmentSentEvidence, SettlementFulfillmentState, SettlementFulfillmentStore,
        SettlementInternalEvidenceSnapshot, canonical_settlement_fulfillment_subject,
    };
    use crate::services::settlement::SettlementFulfillmentService;

    const MAIL_ID: &str = "700001";

    /// 观察时刻恰在 5 分钟追踪窗口边界上，既有节奏是下一个窗口（300 秒后）。
    fn observed_at() -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_100, 0).single().expect("时间戳必须在范围内")
    }

    fn unavailable<T>() -> ApplicationFuture<'static, T> {
        Box::pin(async { Err(ApplicationError::Repository(RepositoryError::Unavailable)) })
    }

    /// 只记录延期与满足写入；本组用例不应触发其他写入。
    #[derive(Default)]
    struct DeferStore {
        deferred: Mutex<Vec<(Duration, String)>>,
        satisfied: Mutex<Vec<SettlementFulfillmentProof>>,
    }

    impl SettlementFulfillmentStore for DeferStore {
        fn claim_due(
            &self,
            _worker_id: String,
            _now: DateTime<Utc>,
            _lease_until: DateTime<Utc>,
            _limit: u16,
        ) -> ApplicationFuture<'_, Vec<SettlementFulfillmentClaim>> {
            unavailable()
        }

        fn prepare_delivery<'a>(
            &'a self,
            _claim: SettlementFulfillmentClaim,
            _preparation: SettlementDeliveryPreparation,
            _prepared_at: DateTime<Utc>,
            _custody: Option<(&'a SettlementInternalEvidenceSnapshot, std::time::Instant)>,
        ) -> ApplicationFuture<'a, SettlementFulfillmentResult> {
            unavailable()
        }

        fn activate_delivery<'a>(
            &'a self,
            _claim: SettlementFulfillmentClaim,
            _activate_command_id: CommandId,
            _activate_request_hash: RequestHash,
            _activated_at: DateTime<Utc>,
            _custody: Option<(&'a SettlementInternalEvidenceSnapshot, std::time::Instant)>,
        ) -> ApplicationFuture<'a, SettlementFulfillmentActivationResult> {
            unavailable()
        }

        fn satisfy(
            &self,
            claim: SettlementFulfillmentClaim,
            proof: SettlementFulfillmentProof,
        ) -> ApplicationFuture<'_, SettlementFulfillmentReconcileResult> {
            if let Ok(mut satisfied) = self.satisfied.lock() {
                satisfied.push(proof);
            }
            let result = SettlementFulfillmentReconcileResult {
                obligation_id: claim.obligation_id,
                delivery_id: claim.delivery_id.unwrap_or_else(Uuid::nil),
                status: SettlementFulfillmentReconcileStatus::Satisfied,
            };
            Box::pin(async move { Ok(result) })
        }

        fn manual_reconcile(
            &self,
            _claim: SettlementFulfillmentClaim,
            _failure_code: String,
        ) -> ApplicationFuture<'_, ()> {
            unavailable()
        }

        fn mark_failed(
            &self,
            _claim: SettlementFulfillmentClaim,
            _failure_code: String,
        ) -> ApplicationFuture<'_, ()> {
            unavailable()
        }

        fn fail(
            &self,
            _claim: SettlementFulfillmentClaim,
            retry_delay: Duration,
            failure_code: String,
        ) -> ApplicationFuture<'_, ()> {
            if let Ok(mut deferred) = self.deferred.lock() {
                deferred.push((retry_delay, failure_code));
            }
            Box::pin(async { Ok(()) })
        }
    }

    struct NoRecipient;

    impl SettlementFulfillmentRecipientReader for NoRecipient {
        fn read_recipient(
            &self,
            _user_id: Uuid,
        ) -> ApplicationFuture<'_, SettlementFulfillmentRecipientSnapshot> {
            unavailable()
        }
    }

    /// 买家身份读取：返回与 claim 相同的用户；enqueued 阶段只核对身份，不要求当前资格。
    struct BuyerRecipient;

    impl SettlementFulfillmentRecipientReader for BuyerRecipient {
        fn read_recipient(
            &self,
            user_id: Uuid,
        ) -> ApplicationFuture<'_, SettlementFulfillmentRecipientSnapshot> {
            Box::pin(async move {
                Ok(SettlementFulfillmentRecipientSnapshot {
                    user_id,
                    game_identity: "collection-buyer".to_owned(),
                    eligible: true,
                    state_version: 1,
                })
            })
        }
    }

    struct NoCustody;

    impl SettlementFulfillmentCustodyReader for NoCustody {
        fn read_custody(
            &self,
            _request: InternalEvidenceRequest,
        ) -> InternalEvidenceFuture<'_, PreparedSettlementInternalEvidence> {
            Box::pin(async { Err(InternalEvidenceError::Unavailable) })
        }
    }

    /// Sent 读取结果：已读到未领取、Sent 中找不到、远端失败或本地依赖失败。
    #[derive(Clone, Copy)]
    enum SentRead {
        Uncollected,
        NotObserved,
        Remote,
        Local,
    }

    struct ScriptedSent {
        read: SentRead,
        evidence: SettlementFulfillmentSentEvidence,
    }

    impl SettlementFulfillmentEvidenceReader for ScriptedSent {
        fn read_sent(
            &self,
            _world: SettlementWorld,
            _external_mail_id: String,
        ) -> ApplicationFuture<'_, SettlementFulfillmentSentEvidence> {
            let result = match self.read {
                SentRead::Uncollected => Ok(self.evidence.clone()),
                SentRead::NotObserved => Err(ApplicationError::FulfillmentEvidenceNotObserved),
                SentRead::Remote => Err(ApplicationError::RemoteDependency {
                    kind: SafeErrorKind::REMOTE_PROTOCOL_ERROR,
                    detail: "MoogleMail Sent 读取失败".to_owned(),
                }),
                SentRead::Local => Err(ApplicationError::Repository(RepositoryError::Unavailable)),
            };
            Box::pin(async move { result })
        }

        fn read_balance(
            &self,
            _world: SettlementWorld,
            _observed_at: DateTime<Utc>,
        ) -> ApplicationFuture<'_, SettlementFulfillmentBalanceEvidence> {
            unavailable()
        }
    }

    struct FixedClock(DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    /// 已寄出、未领取的卖方货款 claim；投递快照与 canonical 意图一致，`sent_at` 为投递完成时刻。
    fn sent_payout(sent_at: Option<DateTime<Utc>>) -> SettlementFulfillmentClaim {
        let obligation_id = SettlementId::from_uuid(Uuid::from_u128(0x1001));
        let claimed_at = observed_at() - Duration::seconds(1);
        let mut claim = SettlementFulfillmentClaim {
            obligation_id,
            auction_id: AuctionId::from_uuid(Uuid::from_u128(2)),
            lot_id: LotId::from_uuid(Uuid::from_u128(3)),
            origin_command_id: CommandId::from_uuid(Uuid::from_u128(4)),
            world: SettlementWorld::Main,
            kind: SettlementObligationKind::SellerPayout,
            state: SettlementFulfillmentState::Enqueued,
            operation: SettlementFulfillmentOperation::Send,
            recipient_user_id: None,
            recipient: "collection-seller".to_owned(),
            amount: 900,
            carrier_credit: 0,
            equipment_id: EquipmentId::from_uuid(Uuid::from_u128(6)),
            equipment_world: SettlementWorld::Main,
            external_equipment_id: "317632875".to_owned(),
            equipment_name: "Radiant Phase Shoes".to_owned(),
            settlement_line: None,
            settlement_mode: None,
            payment_deadline_at: None,
            submission_id: None,
            submission_source_kind: None,
            submission_external_mail_id: None,
            prerequisite_id: Some(SettlementId::from_uuid(Uuid::from_u128(0x1000))),
            delivery_id: Some(stable_delivery_id(obligation_id)),
            prepare_command_id: Some(stable_prepare_command_id(obligation_id)),
            prepare_request_hash: None,
            activate_command_id: Some(CommandId::from_uuid(Uuid::from_u128(0x2001))),
            activate_request_hash: Some(RequestHash::new([8; 32])),
            delivery_snapshot: None,
            claim_token: Uuid::from_u128(7),
            claim_fencing: 1,
            state_version: 3,
            worker_id: "worker".to_owned(),
            claimed_at,
            lease_until: claimed_at + Duration::seconds(60),
            retry_count: 4,
        };
        let canonical = build_delivery_intent(&claim)
            .and_then(|intent| build_canonical_delivery(&intent))
            .expect("货款意图必须可规范化");
        claim.prepare_request_hash = Some(canonical.request_hash);
        claim.delivery_snapshot = Some(SettlementFulfillmentDeliverySnapshot {
            id: stable_delivery_id(obligation_id),
            status: SettlementFulfillmentDeliveryStatus::Succeeded,
            prepare_command_id: stable_prepare_command_id(obligation_id),
            prepare_request_hash: canonical.request_hash,
            activate_command_id: claim.activate_command_id,
            activate_request_hash: claim.activate_request_hash,
            origin_kind: "settlement".to_owned(),
            origin_id: obligation_id.to_string(),
            recipient: claim.recipient.clone(),
            payload: canonical.payload,
            external_reference: Some(format!("mooglemail:main:send:{MAIL_ID}")),
            external_reference_proven: true,
            failure_code: None,
            completed_at: sent_at,
        });
        claim
    }

    /// 只轮询立即就绪的假端口，不引入额外异步运行时依赖。
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(value) => return value,
                std::task::Poll::Pending => std::hint::spin_loop(),
            }
        }
    }

    /// 执行一次 enqueued 对账，返回持久化延期的间隔与原因码。
    fn defer_after(read: SentRead, sent_at: Option<DateTime<Utc>>) -> (Duration, String) {
        let claim = sent_payout(sent_at);
        let evidence = SettlementFulfillmentSentEvidence {
            external_mail_id: MAIL_ID.to_owned(),
            world: SettlementWorld::Main,
            recipient: claim.recipient.clone(),
            subject: canonical_settlement_fulfillment_subject(claim.kind, claim.obligation_id)
                .expect("货款主题"),
            remaining_credits: claim.amount,
            cod_pending: 0,
            remaining_equipment_ids: Vec::new(),
            collected: false,
            observed_at: observed_at(),
        };
        let store = Arc::new(DeferStore::default());
        let service = SettlementFulfillmentService::new(
            store.clone(),
            Arc::new(NoRecipient),
            Arc::new(NoCustody),
            Arc::new(ScriptedSent { read, evidence }),
            Arc::new(FixedClock(observed_at())),
        );
        let result = block_on(service.reconcile_now(
            claim,
            observed_at(),
            observed_at() + Duration::seconds(60),
        ))
        .expect("对账必须持久化延期");
        assert_eq!(result.status, SettlementFulfillmentReconcileStatus::Deferred);
        let deferred = store.deferred.lock().expect("延期记录").clone();
        assert_eq!(deferred.len(), 1, "每次对账只写一次延期");
        deferred[0].clone()
    }

    /// 已读到未领取：寄出不足 24 小时保持 5 分钟追踪窗口，满 24 小时每小时一次，满 7 天每 6 小时；
    /// 投递完成时刻缺失时不放宽。
    #[test]
    fn uncollected_payout_polls_by_time_since_sent() {
        let window = Duration::seconds(300);
        for (sent_at, expected) in [
            (Some(observed_at() - Duration::hours(23)), window),
            (Some(observed_at() - Duration::hours(24)), Duration::hours(1)),
            (Some(observed_at() - Duration::days(7)), Duration::hours(6)),
            (None, window),
        ] {
            let (delay, code) = defer_after(SentRead::Uncollected, sent_at);
            assert_eq!((delay, code.as_str()), (expected, "evidence_not_observed"), "{sent_at:?}");
        }
    }

    /// Sent 找不到与远端读取失败同样按寄出时长放宽；本地依赖失败仍按配置间隔尽快重试。
    #[test]
    fn sent_read_outcomes_follow_collection_tiers_except_local_failures() {
        let sent_at = Some(observed_at() - Duration::days(8));
        assert_eq!(
            defer_after(SentRead::NotObserved, sent_at),
            (Duration::hours(6), "evidence_not_observed".to_owned())
        );
        assert_eq!(
            defer_after(SentRead::Remote, sent_at),
            (Duration::hours(6), "evidence_unavailable".to_owned())
        );
        assert_eq!(
            defer_after(SentRead::Local, sent_at),
            (Duration::seconds(60), "evidence_unavailable".to_owned())
        );
        // 寄出不足 24 小时：远端失败仍按年龄退避（retry_count = 4 时为 10 分钟上限）。
        assert_eq!(
            defer_after(SentRead::Remote, Some(observed_at() - Duration::hours(2))),
            (Duration::minutes(10), "evidence_unavailable".to_owned())
        );
    }

    /// 把 claim 的投递快照改为由当前意图重新规范化的载荷，并同步 prepare 摘要与收件人。
    fn with_canonical_snapshot(
        mut claim: SettlementFulfillmentClaim,
    ) -> SettlementFulfillmentClaim {
        let canonical = build_delivery_intent(&claim)
            .and_then(|intent| build_canonical_delivery(&intent))
            .expect("付款意图必须可规范化");
        claim.prepare_request_hash = Some(canonical.request_hash);
        let recipient = claim.recipient.clone();
        let snapshot = claim.delivery_snapshot.as_mut().expect("投递快照");
        snapshot.prepare_request_hash = canonical.request_hash;
        snapshot.recipient = recipient;
        snapshot.payload = canonical.payload;
        claim
    }

    /// 已寄出、等待买家付款的 COD 或载体付款 claim：投递已成功、Sent 引用已证明（发送证明），
    /// 带冻结付款期限。COD 沿用三变量旧载荷，免去冻结结算明细；载体付款带 1 Credit 载体。
    fn sent_payment(
        kind: SettlementObligationKind,
        sent_at: DateTime<Utc>,
        payment_deadline_at: DateTime<Utc>,
    ) -> SettlementFulfillmentClaim {
        let mut claim = sent_payout(Some(sent_at));
        claim.kind = kind;
        claim.recipient_user_id = Some(Uuid::from_u128(0x3001));
        claim.recipient = "collection-buyer".to_owned();
        claim.amount = 1_000;
        claim.prerequisite_id = None;
        claim.payment_deadline_at = Some(payment_deadline_at);
        if kind == SettlementObligationKind::WinnerPaymentCarrierCod {
            claim.carrier_credit = 1;
        } else if let Some(snapshot) = claim.delivery_snapshot.as_mut() {
            snapshot.payload = serde_json::json!({
                "variables": {"amount": "", "item_name": "", "reference": ""}
            });
        }
        with_canonical_snapshot(claim)
    }

    /// 执行一次 enqueued 对账，返回结果状态、持久化延期与满足证明；`collected` 决定 Sent 记录是否
    /// 已被收件人领取（买家已付款）。
    fn reconcile_payment(
        claim: SettlementFulfillmentClaim,
        read: SentRead,
        collected: bool,
    ) -> (
        SettlementFulfillmentReconcileStatus,
        Vec<(Duration, String)>,
        Vec<SettlementFulfillmentProof>,
    ) {
        let (remaining_credits, cod_pending, remaining_equipment_ids) = match claim.kind {
            _ if collected => (0, 0, Vec::new()),
            SettlementObligationKind::WinnerPaymentCarrierCod => (
                i64::from(claim.carrier_credit),
                claim.amount + i64::from(claim.carrier_credit),
                Vec::new(),
            ),
            _ => (0, claim.amount, vec![claim.external_equipment_id.clone()]),
        };
        let evidence = SettlementFulfillmentSentEvidence {
            external_mail_id: MAIL_ID.to_owned(),
            world: SettlementWorld::Main,
            recipient: claim.recipient.clone(),
            subject: canonical_settlement_fulfillment_subject(claim.kind, claim.obligation_id)
                .expect("付款主题"),
            remaining_credits,
            cod_pending,
            remaining_equipment_ids,
            collected,
            observed_at: observed_at(),
        };
        let store = Arc::new(DeferStore::default());
        let service = SettlementFulfillmentService::new(
            store.clone(),
            Arc::new(BuyerRecipient),
            Arc::new(NoCustody),
            Arc::new(ScriptedSent { read, evidence }),
            Arc::new(FixedClock(observed_at())),
        );
        let result = block_on(service.reconcile_now(
            claim,
            observed_at(),
            observed_at() + Duration::seconds(60),
        ))
        .expect("对账必须写入结果");
        let deferred = store.deferred.lock().expect("延期记录").clone();
        let satisfied = store.satisfied.lock().expect("满足记录").clone();
        (result.status, deferred, satisfied)
    }

    /// 正常 COD 等待付款与载体付款的完整对账路径：投递已寄出且带发送证明，买家未付款时按既有的
    /// 5 分钟追踪窗口或证据退避读取，寄出多久都不按分档放宽（付款期限内与期限早已过去都一样）；
    /// 买家付款后由领取证明满足义务。
    #[test]
    fn cod_and_carrier_payments_keep_existing_cadence_through_reconcile() {
        let window = Duration::seconds(300);
        let backoff = Duration::minutes(10);
        for (kind, sent_at, deadline) in [
            (
                SettlementObligationKind::WinnerEquipmentCod,
                observed_at() - Duration::days(2),
                observed_at() + Duration::days(1),
            ),
            (
                SettlementObligationKind::WinnerPaymentCarrierCod,
                observed_at() - Duration::days(2),
                observed_at() + Duration::days(1),
            ),
            (
                SettlementObligationKind::WinnerEquipmentCod,
                observed_at() - Duration::days(8),
                observed_at() - Duration::days(5),
            ),
            (
                SettlementObligationKind::WinnerPaymentCarrierCod,
                observed_at() - Duration::days(8),
                observed_at() - Duration::days(5),
            ),
        ] {
            let claim = || sent_payment(kind, sent_at, deadline);
            for (read, expected) in [
                (SentRead::Uncollected, (window, "evidence_not_observed")),
                (SentRead::NotObserved, (backoff, "evidence_not_observed")),
                (SentRead::Remote, (backoff, "evidence_unavailable")),
            ] {
                let (status, deferred, satisfied) = reconcile_payment(claim(), read, false);
                assert_eq!(status, SettlementFulfillmentReconcileStatus::Deferred);
                assert_eq!(
                    deferred,
                    [(expected.0, expected.1.to_owned())],
                    "{kind:?} 寄出于 {sent_at} 不按分档放宽"
                );
                assert!(satisfied.is_empty());
            }
            let (status, deferred, satisfied) =
                reconcile_payment(claim(), SentRead::Uncollected, true);
            assert_eq!(status, SettlementFulfillmentReconcileStatus::Satisfied);
            assert!(deferred.is_empty());
            assert_eq!(satisfied.len(), 1, "{kind:?} 买家付款后由领取证明满足");
            assert_eq!(satisfied[0].kind, "mooglemail_sent_collected");
            assert_eq!(satisfied[0].reference, MAIL_ID);
        }
    }
}
