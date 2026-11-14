//! 结算履约 `enqueued` 阶段：读取 MoogleMail Sent 证据并推进对账终态。

use chrono::{DateTime, Utc};

use super::authorization::{AuthorizationResult, authorize_enqueued_recipient};
use super::evidence::validate_sent_evidence;
use super::intent::build_delivery_intent;
use super::proof::proof_digest;
use super::schedule::{collection_poll_delay, evidence_poll_delay, evidence_read_failure_delay};
use super::snapshot::delivery_snapshot_matches;
use super::validation::{repository_error, valid_external_mail_id, validate_claim};
use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, SafeErrorKind, diagnostic_id, record_error,
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
                // 发送与履约是独立循环；先释放本阶段 claim，再以同一事务写入追踪时间。
                // COD/货款沿用共享窗口，其余装备寄送按证据年龄退避；付款截止前后收窄到基础间隔。
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
                        obligation_id: diagnostic_id(&claim.obligation_id),
                        world: Some(claim.world),
                        defer_reason: Some("evidence_not_observed"),
                        ..Default::default()
                    },
                );
                return self
                    .defer_now(
                        claim,
                        delivery_id,
                        poll_delay,
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
                record_error(
                    DiagnosticLevel::Info,
                    SafeErrorKind::EVIDENCE_NOT_OBSERVED,
                    "fulfillment",
                    "wait_sent_receipt",
                    DiagnosticContext {
                        obligation_id: diagnostic_id(&claim.obligation_id),
                        world: Some(claim.world),
                        defer_reason: Some("evidence_not_observed"),
                        ..Default::default()
                    },
                );
                return self
                    .defer_now(
                        claim,
                        delivery_id,
                        evidence_delay,
                        "evidence_not_observed",
                        "MoogleMail Sent 暂未观察到目标邮件",
                    )
                    .await;
            }
            Err(ApplicationError::FulfillmentDependencyMismatch) => {
                return self.manual_reconcile_now(claim, delivery_id, "evidence_mismatch").await;
            }
            Err(error) => {
                // 账号忙是正常退避；其他证据读取错误保留 WARN 供排查。
                let level = match error.diagnostic_kind().label() {
                    "remote_busy" | "remote_account_busy" => DiagnosticLevel::Info,
                    _ => DiagnosticLevel::Warn,
                };
                record_error(
                    level,
                    error.diagnostic_kind(),
                    "fulfillment",
                    "read_sent_receipt",
                    DiagnosticContext {
                        obligation_id: diagnostic_id(&claim.obligation_id),
                        world: Some(claim.world),
                        ..Default::default()
                    },
                );

                let delay = evidence_read_failure_delay(&error, retry_delay, evidence_delay);
                return self
                    .defer_now(
                        claim,
                        delivery_id,
                        delay,
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
            record_error(
                DiagnosticLevel::Info,
                SafeErrorKind::EVIDENCE_NOT_OBSERVED,
                "fulfillment",
                "wait_asset_collection",
                DiagnosticContext {
                    obligation_id: diagnostic_id(&claim.obligation_id),
                    world: Some(claim.world),
                    defer_reason: Some("evidence_not_observed"),
                    ..Default::default()
                },
            );
            // 买家支付 COD、卖家领取货款都只能等对方在游戏内操作；统一对到下一个窗口，
            // 付款截止前后收窄到基础间隔，其余装备寄送按证据年龄退避。
            let poll_delay = collection_poll_delay(
                self.collection_next_poll(&claim, observed_at),
                observed_at,
                evidence_delay,
                claim.payment_deadline_at,
            );
            return self
                .defer_now(
                    claim,
                    delivery_id,
                    poll_delay,
                    "evidence_not_observed",
                    "MoogleMail Sent 邮件仍有待领取资产",
                )
                .await;
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
