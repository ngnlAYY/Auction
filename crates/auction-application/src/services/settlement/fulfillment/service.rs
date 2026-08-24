//! 结算履约 prepare、activate、reconcile 服务编排。

use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use auction_domain::CommandId;

use crate::{
    ApplicationError, ApplicationFuture, RequestHash, SettlementDeliveryPreparation,
    SettlementFulfillmentActivationResult, SettlementFulfillmentClaim,
    SettlementFulfillmentCommandPort, SettlementFulfillmentEvidenceReader,
    SettlementFulfillmentOperation, SettlementFulfillmentProof,
    SettlementFulfillmentReconcileResult, SettlementFulfillmentReconcileStatus,
    SettlementFulfillmentResult, SettlementFulfillmentState, SettlementFulfillmentStore,
};

use super::evidence::{validate_balance_evidence, validate_sent_evidence};
use super::intent::{build_delivery_intent, stable_id};
use super::model::SettlementDeliveryActivation;
use super::proof::proof_digest;
use super::validation::{
    expected_credit_amount, repository_error, valid_external_mail_id, validate_claim,
};

/// 结算履约应用服务。
///
/// 服务只把可执行的 `pending` 义务制作为 prepared delivery outbox，不读取密钥、不发 HTTP，
/// 也不把 `prepared` 误报为外部发送成功；delivery Worker 负责后续激活、发送和证据回写。
pub struct SettlementFulfillmentService {
    store: Arc<dyn SettlementFulfillmentStore>,
    evidence_reader: Arc<dyn SettlementFulfillmentEvidenceReader>,
}

impl SettlementFulfillmentService {
    /// 绑定履约存储端口，保持 Worker 装配根不依赖具体数据库类型。
    pub fn new(
        store: Arc<dyn SettlementFulfillmentStore>,
        evidence_reader: Arc<dyn SettlementFulfillmentEvidenceReader>,
    ) -> Self {
        Self { store, evidence_reader }
    }

    async fn prepare_now(
        &self,
        claim: SettlementFulfillmentClaim,
        prepared_at: DateTime<Utc>,
    ) -> Result<SettlementFulfillmentResult, ApplicationError> {
        validate_claim(&claim)?;
        if claim.state != SettlementFulfillmentState::Pending {
            return Err(repository_error("当前履约阶段不允许生成 prepared delivery"));
        }

        let intent = build_delivery_intent(&claim)?;
        if intent.credits > 0 {
            let balance = self.evidence_reader.read_balance(claim.world).await?;
            if balance.world != claim.world
                || balance.credits < intent.credits
                || balance.observed_at < claim.claimed_at
                || balance.observed_at > prepared_at
            {
                return Err(repository_error("MoogleMail 发送前余额证明不足或已过期"));
            }
        }
        let request_hash = RequestHash::from_request(
            "prepare_settlement_delivery",
            claim.obligation_id.as_uuid(),
            &intent,
        )?;
        let payload = serde_json::to_value(&intent)?;
        let preparation = SettlementDeliveryPreparation {
            delivery_id: stable_id(claim.obligation_id, "settlement-delivery", 1),
            prepare_command_id: CommandId::from_uuid(stable_id(
                claim.obligation_id,
                "settlement-delivery-prepare",
                1,
            )),
            request_hash,
            recipient: claim.recipient.clone(),
            payload,
        };
        self.store.prepare_delivery(claim, preparation, prepared_at).await
    }

    async fn activate_now(
        &self,
        claim: SettlementFulfillmentClaim,
        activated_at: DateTime<Utc>,
    ) -> Result<SettlementFulfillmentActivationResult, ApplicationError> {
        validate_claim(&claim)?;
        let (Some(delivery_id), Some(_prepare_command_id), Some(prepare_request_hash)) =
            (claim.delivery_id, claim.prepare_command_id, claim.prepare_request_hash)
        else {
            return Err(repository_error("prepared 履约缺少 delivery prepare identity"));
        };
        if claim.state != SettlementFulfillmentState::Prepared {
            return Err(repository_error("当前履约阶段不允许激活 delivery"));
        }
        let activation_command_id =
            CommandId::from_uuid(stable_id(claim.obligation_id, "settlement-delivery-activate", 1));
        let activation = SettlementDeliveryActivation {
            schema_version: 1,
            delivery_id,
            obligation_id: claim.obligation_id.as_uuid(),
            prepare_request_hash: prepare_request_hash.as_bytes(),
            expected_status: "prepared",
        };
        let activation_request_hash =
            RequestHash::from_request("activate_settlement_delivery", delivery_id, &activation)?;
        self.store
            .activate_delivery(claim, activation_command_id, activation_request_hash, activated_at)
            .await
    }

    async fn reconcile_now(
        &self,
        claim: SettlementFulfillmentClaim,
    ) -> Result<SettlementFulfillmentReconcileResult, ApplicationError> {
        validate_claim(&claim)?;
        if claim.state != SettlementFulfillmentState::Enqueued
            || claim.operation != SettlementFulfillmentOperation::Send
        {
            return Err(repository_error("当前履约阶段不允许读取发送回执"));
        }
        let Some(delivery_id) = claim.delivery_id else {
            return Err(repository_error("发送回执缺少 delivery identity"));
        };
        let Some(external_reference) = claim.external_reference.as_deref() else {
            return self
                .manual_reconcile_now(claim, delivery_id, "external_reference_unproven")
                .await;
        };
        if !claim.external_reference_proven
            || !valid_external_mail_id(external_reference)
            || external_reference.starts_with("delivery:")
        {
            return self
                .manual_reconcile_now(claim, delivery_id, "external_reference_unproven")
                .await;
        }

        let sent = match self
            .evidence_reader
            .read_sent(claim.world, external_reference.to_owned())
            .await
        {
            Ok(sent) => sent,
            Err(ApplicationError::FulfillmentEvidenceNotObserved) => {
                return self.manual_reconcile_now(claim, delivery_id, "sent_not_observed").await;
            }
            Err(error) => return Err(error),
        };
        validate_sent_evidence(&claim, external_reference, &sent)?;
        if !sent.collected {
            return Err(repository_error("MoogleMail Sent 邮件仍有待领取资产"));
        }

        let balance = if expected_credit_amount(&claim) > 0 {
            Some(self.evidence_reader.read_balance(claim.world).await?)
        } else {
            None
        };
        if let Some(balance) = balance {
            validate_balance_evidence(&claim, &balance)?;
        }
        let proof = SettlementFulfillmentProof {
            kind: "mooglemail_sent_collected".to_owned(),
            reference: external_reference.to_owned(),
            digest: proof_digest(&claim, delivery_id, &sent, balance.as_ref())?,
            observed_at: sent.observed_at,
        };
        self.store.satisfy(claim, proof).await
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
    fn prepare(
        &self,
        claim: SettlementFulfillmentClaim,
        prepared_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, SettlementFulfillmentResult> {
        Box::pin(async move { self.prepare_now(claim, prepared_at).await })
    }

    fn activate(
        &self,
        claim: SettlementFulfillmentClaim,
        activated_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, SettlementFulfillmentActivationResult> {
        Box::pin(async move { self.activate_now(claim, activated_at).await })
    }

    fn reconcile(
        &self,
        claim: SettlementFulfillmentClaim,
    ) -> ApplicationFuture<'_, SettlementFulfillmentReconcileResult> {
        Box::pin(async move { self.reconcile_now(claim).await })
    }
}
