//! 结算履约 prepare、activate、reconcile 服务编排。

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use crate::{
    ApplicationError, ApplicationFuture, Clock, SettlementDeliveryPreparation,
    SettlementFulfillmentActivationResult, SettlementFulfillmentClaim,
    SettlementFulfillmentCommandPort, SettlementFulfillmentCustodyReader,
    SettlementFulfillmentDeliveryStatus, SettlementFulfillmentEvidenceReader,
    SettlementFulfillmentOperation, SettlementFulfillmentPhaseResult, SettlementFulfillmentProof,
    SettlementFulfillmentRecipientReader, SettlementFulfillmentReconcileResult,
    SettlementFulfillmentReconcileStatus, SettlementFulfillmentResult, SettlementFulfillmentState,
    SettlementFulfillmentStore,
};

use super::authorization::{
    AuthorizationResult, authorize_enqueued_recipient, authorize_pending_or_prepared,
};
use super::evidence::validate_sent_evidence;
use super::intent::{
    activation_request_hash, build_canonical_delivery, build_delivery_intent,
    settlement_world_name, stable_activate_command_id, stable_delivery_id,
    stable_prepare_command_id,
};
use super::model::SettlementDeliveryIntent;
use super::proof::proof_digest;
use super::validation::{repository_error, valid_external_mail_id, validate_claim};

/// 结算履约应用服务。
/// 服务只把可执行的 `pending` 义务制作为 prepared delivery outbox，不读取密钥、不发 HTTP，
/// 也不把 `prepared` 误报为外部发送成功；delivery Worker 负责后续激活、发送和证据回写。
pub struct SettlementFulfillmentService {
    pub(super) store: Arc<dyn SettlementFulfillmentStore>,
    pub(super) recipient_reader: Arc<dyn SettlementFulfillmentRecipientReader>,
    custody_reader: Arc<dyn SettlementFulfillmentCustodyReader>,
    evidence_reader: Arc<dyn SettlementFulfillmentEvidenceReader>,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) internal: Option<(
        Arc<dyn crate::SettlementInternalFulfillmentStore>,
        Arc<dyn crate::SettlementInternalEvidenceReader>,
    )>,
}

impl SettlementFulfillmentService {
    /// 绑定履约存储端口，保持 Worker 装配根不依赖具体数据库类型。
    pub fn new(
        store: Arc<dyn SettlementFulfillmentStore>,
        recipient_reader: Arc<dyn SettlementFulfillmentRecipientReader>,
        custody_reader: Arc<dyn SettlementFulfillmentCustodyReader>,
        evidence_reader: Arc<dyn SettlementFulfillmentEvidenceReader>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { store, recipient_reader, custody_reader, evidence_reader, clock, internal: None }
    }

    pub(super) async fn prepare_external(
        &self,
        claim: SettlementFulfillmentClaim,
        prepared_at: DateTime<Utc>,
        retry_delay: Duration,
    ) -> Result<SettlementFulfillmentPhaseResult<SettlementFulfillmentResult>, ApplicationError>
    {
        validate_claim(&claim)?;
        if claim.state != SettlementFulfillmentState::Pending {
            return Err(repository_error("当前履约阶段不允许生成 prepared delivery"));
        }
        let classified_recipient = claim.recipient.clone();
        let (claim, intent) = match authorize_pending_or_prepared(
            self.store.as_ref(),
            self.recipient_reader.as_ref(),
            self.custody_reader.as_ref(),
            self.evidence_reader.as_ref(),
            self.clock.as_ref(),
            claim,
            retry_delay,
        )
        .await?
        {
            AuthorizationResult::Authorized(value) => value,
            AuthorizationResult::Deferred { obligation_id, failure_code } => {
                return Ok(SettlementFulfillmentPhaseResult::Deferred {
                    obligation_id,
                    failure_code,
                });
            }
            AuthorizationResult::ManualReconcile { obligation_id, failure_code } => {
                return Ok(SettlementFulfillmentPhaseResult::ManualReconcile {
                    obligation_id,
                    failure_code,
                });
            }
        };
        if claim.recipient != classified_recipient {
            // 身份分支之后发生改名不能把已证明非bot的旧目标换成新目标直接派发。
            let obligation_id = claim.obligation_id;
            self.store
                .fail(
                    claim,
                    retry_delay,
                    "recipient_unavailable".to_owned(),
                    "身份分支后的收件人已变化".to_owned(),
                )
                .await?;
            return Ok(SettlementFulfillmentPhaseResult::Deferred {
                obligation_id,
                failure_code: "recipient_unavailable",
            });
        }
        let canonical = build_canonical_delivery(&intent)?;
        let preparation = SettlementDeliveryPreparation {
            delivery_id: stable_delivery_id(claim.obligation_id),
            prepare_command_id: stable_prepare_command_id(claim.obligation_id),
            request_hash: canonical.request_hash,
            recipient: claim.recipient.clone(),
            payload: canonical.payload,
        };
        Ok(SettlementFulfillmentPhaseResult::Advanced(
            self.store.prepare_delivery(claim, preparation, prepared_at).await?,
        ))
    }

    async fn activate_now(
        &self,
        claim: SettlementFulfillmentClaim,
        activated_at: DateTime<Utc>,
        retry_delay: Duration,
    ) -> Result<
        SettlementFulfillmentPhaseResult<SettlementFulfillmentActivationResult>,
        ApplicationError,
    > {
        validate_claim(&claim)?;
        if claim.state != SettlementFulfillmentState::Prepared {
            return Err(repository_error("当前履约阶段不允许激活 delivery"));
        }
        let (claim, intent) = match authorize_pending_or_prepared(
            self.store.as_ref(),
            self.recipient_reader.as_ref(),
            self.custody_reader.as_ref(),
            self.evidence_reader.as_ref(),
            self.clock.as_ref(),
            claim,
            retry_delay,
        )
        .await?
        {
            AuthorizationResult::Authorized(value) => value,
            AuthorizationResult::Deferred { obligation_id, failure_code } => {
                return Ok(SettlementFulfillmentPhaseResult::Deferred {
                    obligation_id,
                    failure_code,
                });
            }
            AuthorizationResult::ManualReconcile { obligation_id, failure_code } => {
                return Ok(SettlementFulfillmentPhaseResult::ManualReconcile {
                    obligation_id,
                    failure_code,
                });
            }
        };
        let (Some(delivery_id), Some(_prepare_command_id), Some(prepare_request_hash)) =
            (claim.delivery_id, claim.prepare_command_id, claim.prepare_request_hash)
        else {
            return Err(repository_error("prepared 履约缺少 delivery prepare identity"));
        };
        if !delivery_snapshot_matches(&claim, &intent) {
            let obligation_id = claim.obligation_id;
            self.store.manual_reconcile(claim, "delivery_conflict".to_owned()).await?;
            return Ok(SettlementFulfillmentPhaseResult::ManualReconcile {
                obligation_id,
                failure_code: "delivery_conflict",
            });
        }
        let activation_command_id = stable_activate_command_id(claim.obligation_id);
        let activation_request_hash =
            activation_request_hash(claim.obligation_id, delivery_id, prepare_request_hash)?;
        Ok(SettlementFulfillmentPhaseResult::Advanced(
            self.store
                .activate_delivery(
                    claim,
                    activation_command_id,
                    activation_request_hash,
                    activated_at,
                )
                .await?,
        ))
    }

    async fn reconcile_now(
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
        let Some(delivery_id) = claim.delivery_id else {
            return Err(repository_error("发送回执缺少 delivery identity"));
        };
        if matches!(
            claim.kind,
            auction_domain::SettlementObligationKind::WinnerEquipmentCod
                | auction_domain::SettlementObligationKind::WinnerPaymentCarrierCod
        ) && let Some((store, _)) = &self.internal
        {
            let command = crate::SettlementInternalCommand::new(claim.clone(), None, true)?;
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
        let (delivery_status, external_reference, external_reference_proven) = {
            let snapshot = claim
                .delivery_snapshot
                .as_ref()
                .ok_or_else(|| repository_error("发送回执缺少 delivery 快照"))?;
            (
                snapshot.status,
                snapshot.external_reference.clone(),
                snapshot.external_reference_proven,
            )
        };
        match delivery_status {
            SettlementFulfillmentDeliveryStatus::Pending
            | SettlementFulfillmentDeliveryStatus::Claimed
            | SettlementFulfillmentDeliveryStatus::Sending => {
                return self
                    .defer_now(
                        claim,
                        delivery_id,
                        retry_delay,
                        "evidence_not_observed",
                        "delivery 尚未进入可对账终态",
                    )
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

        let sent = match self.evidence_reader.read_sent(claim.world, remote_id.to_owned()).await {
            Ok(sent) => sent,
            Err(ApplicationError::FulfillmentEvidenceNotObserved) => {
                return self
                    .defer_now(
                        claim,
                        delivery_id,
                        retry_delay,
                        "evidence_not_observed",
                        "MoogleMail Sent 暂未观察到目标邮件",
                    )
                    .await;
            }
            Err(ApplicationError::FulfillmentDependencyMismatch) => {
                return self.manual_reconcile_now(claim, delivery_id, "evidence_mismatch").await;
            }
            Err(_) => {
                return self
                    .defer_now(
                        claim,
                        delivery_id,
                        retry_delay,
                        "evidence_unavailable",
                        "MoogleMail Sent 证据读取暂不可用",
                    )
                    .await;
            }
        };
        let checked_at = self.clock.now();
        if sent.observed_at > checked_at
            || validate_sent_evidence(&claim, remote_id, &sent).is_err()
        {
            return self.manual_reconcile_now(claim, delivery_id, "evidence_mismatch").await;
        }
        if !sent.collected {
            return self
                .defer_now(
                    claim,
                    delivery_id,
                    retry_delay,
                    "evidence_not_observed",
                    "MoogleMail Sent 邮件仍有待领取资产",
                )
                .await;
        }
        let proof = SettlementFulfillmentProof {
            kind: "mooglemail_sent_collected".to_owned(),
            reference: remote_id.to_owned(),
            digest: proof_digest(&claim, delivery_id, &sent, None)?,
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

    async fn defer_now(
        &self,
        claim: SettlementFulfillmentClaim,
        delivery_id: Uuid,
        retry_delay: Duration,
        failure_code: &str,
        safe_detail: &str,
    ) -> Result<SettlementFulfillmentReconcileResult, ApplicationError> {
        let result = SettlementFulfillmentReconcileResult {
            obligation_id: claim.obligation_id,
            delivery_id,
            status: SettlementFulfillmentReconcileStatus::Deferred,
        };
        self.store
            .fail(claim, retry_delay, failure_code.to_owned(), safe_detail.to_owned())
            .await?;
        Ok(result)
    }

    async fn failed_now(
        &self,
        claim: SettlementFulfillmentClaim,
        delivery_id: Uuid,
        failure_code: &str,
    ) -> Result<SettlementFulfillmentReconcileResult, ApplicationError> {
        let result = SettlementFulfillmentReconcileResult {
            obligation_id: claim.obligation_id,
            delivery_id,
            status: SettlementFulfillmentReconcileStatus::Failed,
        };
        self.store.mark_failed(claim, failure_code.to_owned()).await?;
        Ok(result)
    }

    async fn manual_reconcile_now(
        &self,
        claim: SettlementFulfillmentClaim,
        delivery_id: Uuid,
        failure_code: &str,
    ) -> Result<SettlementFulfillmentReconcileResult, ApplicationError> {
        let result = SettlementFulfillmentReconcileResult {
            obligation_id: claim.obligation_id,
            delivery_id,
            status: SettlementFulfillmentReconcileStatus::ManualReconcile,
        };
        self.store.manual_reconcile(claim, failure_code.to_owned()).await?;
        Ok(result)
    }
}

impl SettlementFulfillmentCommandPort for SettlementFulfillmentService {
    fn prepare_next(
        &self,
        claim: SettlementFulfillmentClaim,
        prepared_at: DateTime<Utc>,
        retry_delay: Duration,
    ) -> ApplicationFuture<'_, crate::SettlementFulfillmentPreparationOutcome> {
        Box::pin(async move { self.prepare_classified(claim, prepared_at, retry_delay).await })
    }

    fn prepare(
        &self,
        claim: SettlementFulfillmentClaim,
        prepared_at: DateTime<Utc>,
        retry_delay: Duration,
    ) -> ApplicationFuture<'_, SettlementFulfillmentPhaseResult<SettlementFulfillmentResult>> {
        Box::pin(async move {
            match self.prepare_classified(claim, prepared_at, retry_delay).await? {
                crate::SettlementFulfillmentPreparationOutcome::External(result) => Ok(*result),
                crate::SettlementFulfillmentPreparationOutcome::InternallySatisfied { .. } => {
                    // 旧调用者没有无 delivery 成功类型，明确拒绝伪造身份；新版 Worker 使用 prepare_next。
                    Err(repository_error(
                        "义务已内部满足，请使用 prepare_next 读取无 delivery 结果",
                    ))
                }
            }
        })
    }

    fn activate(
        &self,
        claim: SettlementFulfillmentClaim,
        activated_at: DateTime<Utc>,
        retry_delay: Duration,
    ) -> ApplicationFuture<
        '_,
        SettlementFulfillmentPhaseResult<SettlementFulfillmentActivationResult>,
    > {
        Box::pin(async move { self.activate_now(claim, activated_at, retry_delay).await })
    }

    fn reconcile(
        &self,
        claim: SettlementFulfillmentClaim,
        observed_at: DateTime<Utc>,
        retry_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, SettlementFulfillmentReconcileResult> {
        Box::pin(async move { self.reconcile_now(claim, observed_at, retry_at).await })
    }
}

fn delivery_snapshot_matches(
    claim: &SettlementFulfillmentClaim,
    intent: &SettlementDeliveryIntent,
) -> bool {
    let (Some(delivery_id), Some(prepare_command_id), Some(prepare_request_hash)) =
        (claim.delivery_id, claim.prepare_command_id, claim.prepare_request_hash)
    else {
        return false;
    };
    let Some(snapshot) = claim.delivery_snapshot.as_ref() else {
        return false;
    };
    let Ok(expected) = build_canonical_delivery(intent) else {
        return false;
    };
    snapshot.id == delivery_id
        && delivery_id == stable_delivery_id(claim.obligation_id)
        && snapshot.prepare_command_id == prepare_command_id
        && prepare_command_id == stable_prepare_command_id(claim.obligation_id)
        && snapshot.prepare_request_hash == prepare_request_hash
        && prepare_request_hash == expected.request_hash
        && snapshot.activate_command_id == claim.activate_command_id
        && snapshot.activate_request_hash == claim.activate_request_hash
        && snapshot.origin_kind == "settlement"
        && snapshot.origin_id == claim.obligation_id.to_string()
        && snapshot.recipient == claim.recipient
        && snapshot.payload == expected.payload
}

fn fulfillment_reference_suffix(
    reference: &str,
    world: auction_domain::SettlementWorld,
    operation: SettlementFulfillmentOperation,
) -> Option<&str> {
    let prefix = format!("mooglemail:{}:{}:", settlement_world_name(world), operation.as_str());
    reference.strip_prefix(&prefix).filter(|value| !value.is_empty() && *value == value.trim())
}
