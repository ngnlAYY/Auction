use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use auction_domain::{CommandId, SettlementObligationKind, SettlementWorld, deterministic_uuid};

use crate::{
    ApplicationError, ApplicationFuture, RepositoryError, RequestHash,
    SettlementDeliveryPreparation, SettlementFulfillmentActivationResult,
    SettlementFulfillmentBalanceEvidence, SettlementFulfillmentClaim,
    SettlementFulfillmentCommandPort, SettlementFulfillmentEvidenceReader,
    SettlementFulfillmentProof, SettlementFulfillmentReconcileResult,
    SettlementFulfillmentReconcileStatus, SettlementFulfillmentResult,
    SettlementFulfillmentSentEvidence, SettlementFulfillmentState, SettlementFulfillmentStore,
};

/// 结算履约交给 delivery owner 的无 secret canonical 意图。
///
/// 这是 owner 间的稳定防腐层，不是 MoogleMail 的最终表单 payload。外部适配器必须在本地
/// outbox 已提交后再把它转换成具体网关协议，避免把旧协议字段未经审计地传播进结算领域。
#[derive(Debug, Serialize)]
struct SettlementDeliveryIntent {
    schema_version: u8,
    operation: &'static str,
    world: &'static str,
    obligation_id: Uuid,
    auction_id: Uuid,
    lot_id: Uuid,
    origin_command_id: Uuid,
    kind: &'static str,
    recipient: String,
    template: &'static str,
    item_name: String,
    external_equipment_id: String,
    amount: i64,
    carrier_credit: u32,
    credits: i64,
    cod: i64,
    equipment_id: Uuid,
    submission_id: Option<Uuid>,
    submission_source_kind: Option<String>,
    source_external_mail_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct SettlementDeliveryActivation {
    schema_version: u8,
    delivery_id: Uuid,
    obligation_id: Uuid,
    prepare_request_hash: [u8; 32],
    expected_status: &'static str,
}

/// 结算履约应用服务。
///
/// 当前首片把可执行的 `pending` 义务制作为 prepared delivery outbox。它不读取密钥、不发
/// HTTP，也不把 `prepared` 误报为外部发送成功；后续 delivery Worker 负责激活、发送和
/// 结算证据回写。
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
            || claim.operation != crate::SettlementFulfillmentOperation::Send
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

fn validate_claim(claim: &SettlementFulfillmentClaim) -> Result<(), ApplicationError> {
    if claim.obligation_id.is_nil()
        || claim.auction_id.is_nil()
        || claim.lot_id.is_nil()
        || claim.origin_command_id.is_nil()
        || claim.equipment_id.is_nil()
        || claim.recipient.is_empty()
        || claim.recipient != claim.recipient.trim()
        || !valid_external_equipment_id(&claim.external_equipment_id)
        || claim.equipment_name.is_empty()
        || claim.equipment_name != claim.equipment_name.trim()
        || claim.amount < 0
        || claim.claim_token.is_nil()
        || claim.claim_fencing <= 0
        || claim.state_version == 0
        || claim.worker_id.is_empty()
        || claim.worker_id != claim.worker_id.trim()
        || claim.claimed_at >= claim.lease_until
        || claim.attempt_count == 0
    {
        return Err(repository_error("数据库返回的履约 claim 不满足不变量"));
    }
    match claim.state {
        SettlementFulfillmentState::Pending
            if claim.delivery_id.is_none()
                && claim.prepare_command_id.is_none()
                && claim.prepare_request_hash.is_none() => {}
        SettlementFulfillmentState::Prepared
            if claim.delivery_id.is_some()
                && claim.prepare_command_id.is_some()
                && claim.prepare_request_hash.is_some_and(|hash| hash.as_bytes() != [0; 32]) => {}
        SettlementFulfillmentState::Enqueued
            if claim.delivery_id.is_some()
                && claim.prepare_command_id.is_some()
                && claim.prepare_request_hash.is_some_and(|hash| hash.as_bytes() != [0; 32]) => {}
        _ => return Err(repository_error("数据库返回的履约阶段 identity 不一致")),
    }
    Ok(())
}

fn build_delivery_intent(
    claim: &SettlementFulfillmentClaim,
) -> Result<SettlementDeliveryIntent, ApplicationError> {
    let mut intent = SettlementDeliveryIntent {
        schema_version: 2,
        operation: claim.operation.as_str(),
        world: settlement_world_name(claim.world),
        obligation_id: claim.obligation_id.as_uuid(),
        auction_id: claim.auction_id.as_uuid(),
        lot_id: claim.lot_id.as_uuid(),
        origin_command_id: claim.origin_command_id.as_uuid(),
        kind: obligation_kind_name(claim.kind),
        recipient: claim.recipient.clone(),
        template: "",
        item_name: String::new(),
        external_equipment_id: claim.external_equipment_id.clone(),
        amount: claim.amount,
        carrier_credit: claim.carrier_credit,
        credits: 0,
        cod: 0,
        equipment_id: claim.equipment_id.as_uuid(),
        submission_id: claim.submission_id.map(|id| id.as_uuid()),
        submission_source_kind: claim.submission_source_kind.clone(),
        source_external_mail_id: claim.submission_external_mail_id.clone(),
    };

    match claim.kind {
        SettlementObligationKind::WinnerEquipmentCod => {
            intent.template = "settlement_winner_equipment_cod";
            intent.item_name = claim.equipment_name.clone();
            intent.cod = claim.amount;
        }
        SettlementObligationKind::WinnerPaymentCarrierCod => {
            intent.template = "settlement_payment_carrier";
            intent.credits = i64::from(claim.carrier_credit);
            intent.cod = claim
                .amount
                .checked_add(intent.credits)
                .ok_or_else(|| repository_error("结算履约 COD 金额溢出"))?;
        }
        SettlementObligationKind::WinnerEquipmentHandoff => {
            intent.template = "settlement_equipment_handoff";
            intent.item_name = claim.equipment_name.clone();
        }
        SettlementObligationKind::SellerPayout => {
            intent.template = "settlement_seller_payout";
            intent.credits = claim.amount;
        }
        SettlementObligationKind::BuyerRefund => {
            intent.template = "settlement_buyer_refund";
            intent.credits = claim.amount;
        }
        SettlementObligationKind::UnsoldReturn | SettlementObligationKind::WithdrawnReturn => {
            match (
                claim.submission_source_kind.as_deref(),
                claim.submission_external_mail_id.as_deref(),
            ) {
                (Some("mail"), Some(external_mail_id))
                    if !external_mail_id.is_empty()
                        && external_mail_id == external_mail_id.trim() =>
                {
                    intent.operation = "return";
                }
                (Some("admin"), None) => {
                    intent.operation = "send";
                    intent.template = "settlement_equipment_handoff";
                    intent.item_name = claim.equipment_name.clone();
                }
                _ => return Err(repository_error("结算退回来源缺少可执行身份")),
            }
        }
    }

    Ok(intent)
}

fn stable_id(obligation_id: auction_domain::SettlementId, label: &str, version: u32) -> Uuid {
    deterministic_uuid(CommandId::from_uuid(obligation_id.as_uuid()), label, version)
}

fn settlement_world_name(world: SettlementWorld) -> &'static str {
    match world {
        SettlementWorld::Main => "main",
        SettlementWorld::Isekai => "isekai",
    }
}

fn obligation_kind_name(kind: SettlementObligationKind) -> &'static str {
    match kind {
        SettlementObligationKind::WinnerEquipmentCod => "winner_equipment_cod",
        SettlementObligationKind::WinnerPaymentCarrierCod => "winner_payment_carrier_cod",
        SettlementObligationKind::WinnerEquipmentHandoff => "winner_equipment_handoff",
        SettlementObligationKind::SellerPayout => "seller_payout",
        SettlementObligationKind::BuyerRefund => "buyer_refund",
        SettlementObligationKind::UnsoldReturn => "unsold_return",
        SettlementObligationKind::WithdrawnReturn => "withdrawn_return",
    }
}

fn repository_error(message: &str) -> ApplicationError {
    RepositoryError::Failed(message.to_owned()).into()
}

fn valid_external_equipment_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_external_mail_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn expected_credit_amount(claim: &SettlementFulfillmentClaim) -> i64 {
    match claim.kind {
        SettlementObligationKind::WinnerPaymentCarrierCod
        | SettlementObligationKind::SellerPayout
        | SettlementObligationKind::BuyerRefund => {
            if claim.kind == SettlementObligationKind::WinnerPaymentCarrierCod {
                i64::from(claim.carrier_credit)
            } else {
                claim.amount
            }
        }
        _ => 0,
    }
}

fn validate_sent_evidence(
    claim: &SettlementFulfillmentClaim,
    external_mail_id: &str,
    evidence: &SettlementFulfillmentSentEvidence,
) -> Result<(), ApplicationError> {
    let expected_subject = match claim.kind {
        SettlementObligationKind::WinnerEquipmentCod
        | SettlementObligationKind::WinnerEquipmentHandoff => "Auction equipment delivery",
        SettlementObligationKind::WinnerPaymentCarrierCod => "Auction payment request",
        SettlementObligationKind::SellerPayout => "Auction seller payout",
        SettlementObligationKind::BuyerRefund => "Auction buyer refund",
        SettlementObligationKind::UnsoldReturn | SettlementObligationKind::WithdrawnReturn => {
            return Err(repository_error("退回义务不应进入发送回执对账"));
        }
    };
    let (remaining_credits, remaining_cod, remaining_equipment) =
        if evidence.collected { (0, 0, Vec::new()) } else { expected_remaining(claim) };
    if evidence.external_mail_id != external_mail_id
        || evidence.world != claim.world
        || !evidence.recipient.eq_ignore_ascii_case(&claim.recipient)
        || evidence.subject != expected_subject
        || evidence.remaining_credits != remaining_credits
        || evidence.cod_pending != remaining_cod
        || evidence.remaining_equipment_ids != remaining_equipment
        || evidence.observed_at < claim.claimed_at
    {
        return Err(repository_error("MoogleMail Sent 证明与结算意图不一致"));
    }
    Ok(())
}

fn expected_remaining(claim: &SettlementFulfillmentClaim) -> (i64, i64, Vec<String>) {
    match claim.kind {
        SettlementObligationKind::WinnerEquipmentCod => {
            (0, claim.amount, vec![claim.external_equipment_id.clone()])
        }
        SettlementObligationKind::WinnerPaymentCarrierCod => (
            i64::from(claim.carrier_credit),
            claim.amount.saturating_add(i64::from(claim.carrier_credit)),
            Vec::new(),
        ),
        SettlementObligationKind::WinnerEquipmentHandoff => {
            (0, 0, vec![claim.external_equipment_id.clone()])
        }
        SettlementObligationKind::SellerPayout | SettlementObligationKind::BuyerRefund => {
            (claim.amount, 0, Vec::new())
        }
        SettlementObligationKind::UnsoldReturn | SettlementObligationKind::WithdrawnReturn => {
            (0, 0, Vec::new())
        }
    }
}

fn validate_balance_evidence(
    claim: &SettlementFulfillmentClaim,
    evidence: &SettlementFulfillmentBalanceEvidence,
) -> Result<(), ApplicationError> {
    if evidence.world != claim.world
        || evidence.credits < 0
        || evidence.observed_at < claim.claimed_at
    {
        return Err(repository_error("MoogleMail Balance 证明无效或过期"));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct SettlementProofDigest<'a> {
    schema_version: u8,
    obligation_id: Uuid,
    delivery_id: Uuid,
    external_mail_id: &'a str,
    world: &'a str,
    recipient: &'a str,
    subject: &'a str,
    remaining_credits: i64,
    cod_pending: i64,
    remaining_equipment_ids: &'a [String],
    collected: bool,
    sent_observed_at: DateTime<Utc>,
    balance_credits: Option<i64>,
    balance_observed_at: Option<DateTime<Utc>>,
}

fn proof_digest(
    claim: &SettlementFulfillmentClaim,
    delivery_id: Uuid,
    sent: &SettlementFulfillmentSentEvidence,
    balance: Option<&SettlementFulfillmentBalanceEvidence>,
) -> Result<[u8; 32], ApplicationError> {
    let payload = SettlementProofDigest {
        schema_version: 1,
        obligation_id: claim.obligation_id.as_uuid(),
        delivery_id,
        external_mail_id: &sent.external_mail_id,
        world: settlement_world_name(sent.world),
        recipient: &sent.recipient,
        subject: &sent.subject,
        remaining_credits: sent.remaining_credits,
        cod_pending: sent.cod_pending,
        remaining_equipment_ids: &sent.remaining_equipment_ids,
        collected: sent.collected,
        sent_observed_at: sent.observed_at,
        balance_credits: balance.map(|value| value.credits),
        balance_observed_at: balance.map(|value| value.observed_at),
    };
    Ok(RequestHash::from_request(
        "settlement_fulfillment_proof",
        claim.obligation_id.as_uuid(),
        &payload,
    )?
    .as_bytes())
}
