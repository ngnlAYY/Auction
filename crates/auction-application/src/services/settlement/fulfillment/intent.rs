//! 结算履约 intent、旧版 schema-v1 payload 与稳定身份。

use std::collections::BTreeMap;

use auction_domain::{
    CommandId, SettlementId, SettlementObligationKind, SettlementWorld, legacy_natural_key_uuid,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    ApplicationError, RequestHash, SettlementFulfillmentClaim, SettlementFulfillmentOperation,
};

use super::canonical::to_go_json_bytes;
use super::model::{SettlementDeliveryActivation, SettlementDeliveryIntent};
use super::validation::repository_error;

const MAXIMUM_PAYLOAD_BYTES: usize = 64 << 10;
const DELIVERY_NAMESPACE: &str = "auction.fulfillment.delivery.v1";
const PREPARE_NAMESPACE: &str = "auction.fulfillment.prepare.v1";
const ACTIVATE_NAMESPACE: &str = "auction.fulfillment.activate.v1";

/// delivery owner 持久化的旧版 canonical payload 与其原始字节摘要。
pub(super) struct CanonicalSettlementDelivery {
    pub(super) payload: serde_json::Value,
    pub(super) request_hash: RequestHash,
}

#[derive(Serialize)]
struct EquipmentSecretReference {
    kind: &'static str,
    id: String,
}

/// 字段声明顺序与旧版 BuildPayload 的匿名 send struct 完全一致。
#[derive(Serialize)]
struct SettlementSendPayload<'a> {
    schema_version: u8,
    operation: &'static str,
    world: &'a str,
    template: &'a str,
    variables: BTreeMap<&'static str, String>,
    credits: i64,
    cod: i64,
    equipment_secret_refs: Vec<EquipmentSecretReference>,
}

/// 根据数据库 claim 生成无 secret 的发送 intent，保留既有 Send 的 canonical 字段顺序。
pub(super) fn build_delivery_intent(
    claim: &SettlementFulfillmentClaim,
) -> Result<SettlementDeliveryIntent, ApplicationError> {
    let mut intent = SettlementDeliveryIntent {
        obligation_id: claim.obligation_id.as_uuid(),
        kind: claim.kind,
        operation: claim.operation.as_str(),
        world: settlement_world_name(claim.world),
        recipient: claim.recipient.clone(),
        template: "",
        reference: String::new(),
        item_name: String::new(),
        credits: 0,
        cod: 0,
        equipment_id: None,
    };

    match claim.kind {
        SettlementObligationKind::WinnerEquipmentCod => {
            intent.template = "settlement_winner_equipment_cod";
            intent.reference = claim.obligation_id.to_string();
            intent.item_name = claim.equipment_name.clone();
            intent.cod = claim.amount;
            intent.equipment_id = Some(claim.equipment_id.as_uuid());
        }
        SettlementObligationKind::WinnerPaymentCarrierCod => {
            intent.template = "settlement_payment_carrier";
            intent.reference = claim.obligation_id.to_string();
            intent.credits = i64::from(claim.carrier_credit);
            intent.cod = claim
                .amount
                .checked_add(intent.credits)
                .ok_or_else(|| repository_error("结算履约 COD 金额溢出"))?;
        }
        SettlementObligationKind::WinnerEquipmentHandoff => {
            intent.template = "settlement_equipment_handoff";
            intent.reference = claim.obligation_id.to_string();
            intent.item_name = claim.equipment_name.clone();
            intent.equipment_id = Some(claim.equipment_id.as_uuid());
        }
        SettlementObligationKind::SellerPayout => {
            intent.template = "settlement_seller_payout";
            intent.reference = claim.obligation_id.to_string();
            intent.credits = claim.amount;
        }
        SettlementObligationKind::BuyerRefund => {
            intent.template = "settlement_buyer_refund";
            intent.reference = claim.obligation_id.to_string();
            intent.credits = claim.amount;
        }
        SettlementObligationKind::UnsoldReturn | SettlementObligationKind::WithdrawnReturn => {
            // 返还必须保持原世界与零金额；旧 Return 或损坏派发事实交由现有错误路径隔离，不能重写请求。
            if claim.operation != SettlementFulfillmentOperation::Send
                || claim.world != claim.equipment_world
                || claim.amount != 0
                || claim.carrier_credit != 0
            {
                return Err(repository_error("装备返还派发事实不符合原装备零金额寄回约束"));
            }
            if !matches!(claim.submission_source_kind.as_deref(), Some("mail" | "admin")) {
                return Err(repository_error("装备返还来源缺少可执行身份"));
            }
            intent.template = "settlement_equipment_handoff";
            intent.reference = claim.obligation_id.to_string();
            intent.item_name = claim.equipment_name.clone();
            intent.equipment_id = Some(claim.equipment_id.as_uuid());
        }
    }

    if !intent.validate() {
        return Err(repository_error("结算履约 intent 组合无效"));
    }
    Ok(intent)
}

/// 把 typed intent 投影为旧版通用 MoogleMail schema-v1 payload，并摘要实际 canonical 字节。
pub(super) fn build_canonical_delivery(
    intent: &SettlementDeliveryIntent,
) -> Result<CanonicalSettlementDelivery, ApplicationError> {
    if !intent.validate() {
        return Err(repository_error("结算履约 intent 组合无效"));
    }
    let encoded = match intent.operation {
        "send" => {
            if !valid_payload_line(&intent.item_name, 512, true) {
                return Err(repository_error("结算履约 payload 文案无效"));
            }
            let mut variables = BTreeMap::from([("reference", intent.reference.clone())]);
            match intent.template {
                "settlement_winner_equipment_cod" => {
                    variables.insert("amount", intent.cod.to_string());
                    variables.insert("item_name", intent.item_name.clone());
                }
                "settlement_payment_carrier" => {
                    variables.insert("amount", intent.cod.to_string());
                }
                "settlement_equipment_handoff" => {
                    variables.insert("item_name", intent.item_name.clone());
                }
                "settlement_seller_payout" | "settlement_buyer_refund" => {
                    variables.insert("amount", intent.credits.to_string());
                }
                _ => return Err(repository_error("结算履约 payload 模板无效")),
            }
            let equipment_secret_refs = intent
                .equipment_id
                .map(|equipment_id| {
                    vec![EquipmentSecretReference {
                        kind: "auction_equipment_key",
                        id: equipment_id.to_string(),
                    }]
                })
                .unwrap_or_default();
            to_go_json_bytes(&SettlementSendPayload {
                schema_version: 1,
                operation: "send",
                world: intent.world,
                template: intent.template,
                variables,
                credits: intent.credits,
                cod: intent.cod,
                equipment_secret_refs,
            })?
        }
        _ => return Err(repository_error("结算履约 payload 操作无效")),
    };
    if encoded.len() > MAXIMUM_PAYLOAD_BYTES {
        return Err(repository_error("结算履约 payload 超出大小上限"));
    }
    let digest = Sha256::digest(&encoded);
    let payload = serde_json::from_slice(&encoded)?;
    Ok(CanonicalSettlementDelivery { payload, request_hash: RequestHash::new(digest.into()) })
}

/// 从 canonical obligation singleton 派生旧版稳定 delivery ID。
pub(super) fn stable_delivery_id(obligation_id: SettlementId) -> Uuid {
    stable_fulfillment_id(DELIVERY_NAMESPACE, obligation_id)
}

/// 从 canonical obligation singleton 派生旧版稳定 prepare command ID。
pub(super) fn stable_prepare_command_id(obligation_id: SettlementId) -> CommandId {
    CommandId::from_uuid(stable_fulfillment_id(PREPARE_NAMESPACE, obligation_id))
}

/// 从 canonical obligation singleton 派生旧版稳定 activate command ID。
pub(super) fn stable_activate_command_id(obligation_id: SettlementId) -> CommandId {
    CommandId::from_uuid(stable_fulfillment_id(ACTIVATE_NAMESPACE, obligation_id))
}

/// 按旧版 FulfillmentActivationRequestDigest 计算无 envelope 的激活摘要。
pub(super) fn activation_request_hash(
    obligation_id: SettlementId,
    delivery_id: Uuid,
    prepare_request_hash: RequestHash,
) -> Result<RequestHash, ApplicationError> {
    let activation = SettlementDeliveryActivation {
        schema_version: 1,
        delivery_id: delivery_id.to_string(),
        obligation_id: obligation_id.to_string(),
        prepare_request_digest: prepare_request_hash.as_bytes(),
        expected_status: "prepared",
    };
    let encoded = to_go_json_bytes(&activation)?;
    Ok(RequestHash::new(Sha256::digest(encoded).into()))
}

fn stable_fulfillment_id(namespace: &str, obligation_id: SettlementId) -> Uuid {
    let obligation_uuid = obligation_id.as_uuid();
    legacy_natural_key_uuid(namespace, &[obligation_uuid.as_bytes()])
}

fn valid_payload_line(value: &str, maximum: usize, allow_empty: bool) -> bool {
    (allow_empty || !value.is_empty())
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

pub(super) fn settlement_world_name(world: SettlementWorld) -> &'static str {
    match world {
        SettlementWorld::Main => "main",
        SettlementWorld::Isekai => "isekai",
    }
}
