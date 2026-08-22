use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use auction_domain::{CommandId, SettlementObligationKind, SettlementWorld, deterministic_uuid};

use crate::{
    ApplicationError, ApplicationFuture, RepositoryError, RequestHash,
    SettlementDeliveryPreparation, SettlementFulfillmentActivationResult,
    SettlementFulfillmentClaim, SettlementFulfillmentCommandPort, SettlementFulfillmentResult,
    SettlementFulfillmentState, SettlementFulfillmentStore,
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
    amount: i64,
    carrier_credit: u32,
    equipment_id: Uuid,
    submission_id: Option<Uuid>,
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
/// 当前首片只把可执行的 `pending` 义务制作为 prepared delivery outbox。它不读取密钥、不发
/// HTTP，也不把 `prepared` 误报为外部发送成功；后续 delivery Worker 负责激活、发送和证据回写。
pub struct SettlementFulfillmentService {
    store: Arc<dyn SettlementFulfillmentStore>,
}

impl SettlementFulfillmentService {
    /// 绑定履约存储端口，保持 Worker 装配根不依赖具体数据库类型。
    pub fn new(store: Arc<dyn SettlementFulfillmentStore>) -> Self {
        Self { store }
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

        let intent = SettlementDeliveryIntent {
            schema_version: 1,
            operation: claim.operation.as_str(),
            world: settlement_world_name(claim.world),
            obligation_id: claim.obligation_id.as_uuid(),
            auction_id: claim.auction_id.as_uuid(),
            lot_id: claim.lot_id.as_uuid(),
            origin_command_id: claim.origin_command_id.as_uuid(),
            kind: obligation_kind_name(claim.kind),
            recipient: claim.recipient.clone(),
            amount: claim.amount,
            carrier_credit: claim.carrier_credit,
            equipment_id: claim.equipment_id.as_uuid(),
            submission_id: claim.submission_id.map(|id| id.as_uuid()),
        };
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
}

fn validate_claim(claim: &SettlementFulfillmentClaim) -> Result<(), ApplicationError> {
    if claim.obligation_id.is_nil()
        || claim.auction_id.is_nil()
        || claim.lot_id.is_nil()
        || claim.origin_command_id.is_nil()
        || claim.equipment_id.is_nil()
        || claim.recipient.is_empty()
        || claim.recipient != claim.recipient.trim()
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
        _ => return Err(repository_error("数据库返回的履约阶段 identity 不一致")),
    }
    Ok(())
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
