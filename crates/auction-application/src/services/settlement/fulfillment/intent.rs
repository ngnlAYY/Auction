//! 结算履约 intent、旧版 schema-v1 payload 与稳定身份。

use std::collections::BTreeMap;

use auction_domain::{
    CommandId, SettlementId, SettlementMode, SettlementObligationKind, legacy_natural_key_uuid,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::canonical::to_go_json_bytes;
use super::model::{SettlementDeliveryActivation, SettlementDeliveryIntent};
use super::validation::repository_error;
use crate::errors::ApplicationError;
use crate::ports::core::RequestHash;
use crate::ports::settlement::{SettlementFulfillmentClaim, SettlementFulfillmentOperation};

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
        world: claim.world.as_str(),
        recipient: claim.recipient.clone(),
        template: "",
        reference: String::new(),
        item_name: String::new(),
        credits: 0,
        cod: 0,
        equipment_id: None,
        settlement_line: None,
        settlement_mode: None,
        payment_deadline_at: claim.payment_deadline_at,
        // 已存在投递只允许按原冻结键集合重放，新增正文变量不得改变在途载荷摘要。
        frozen_variables: claim
            .delivery_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.payload.get("variables"))
            .and_then(serde_json::Value::as_object)
            .map(|variables| variables.keys().cloned().collect()),
    };

    match claim.kind {
        SettlementObligationKind::WinnerEquipmentCod => {
            intent.template = "settlement_winner_equipment_cod";
            intent.reference = claim.obligation_id.to_string();
            intent.item_name = claim.equipment_name.clone();
            intent.cod = claim.amount;
            intent.equipment_id = Some(claim.equipment_id.as_uuid());
            // 已准备/派发的旧任务继续使用原三变量载荷，避免升级后同一 command_id 摘要冲突。
            // 仅精确识别旧变量集；其他未知形状仍由完整 canonical 快照比较拒绝。
            let legacy_payload = claim.delivery_snapshot.as_ref().is_some_and(|snapshot| {
                snapshot
                    .payload
                    .get("variables")
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|variables| {
                        variables.len() == 3
                            && ["amount", "item_name", "reference"]
                                .iter()
                                .all(|key| variables.contains_key(*key))
                    })
            });
            if !legacy_payload {
                let line = claim
                    .settlement_line
                    .as_ref()
                    .filter(|line| line.lot_id == claim.lot_id)
                    .ok_or_else(|| repository_error("竞得装备邮件缺少对应的冻结结算明细"))?;
                intent.settlement_line = Some(line.clone());
                intent.settlement_mode = Some(
                    claim
                        .settlement_mode
                        .ok_or_else(|| repository_error("竞得装备邮件缺少对应的冻结结算模式"))?,
                );
            }
        }
        SettlementObligationKind::WinnerPaymentCarrierCod => {
            intent.template = "settlement_payment_carrier";
            intent.reference = claim.obligation_id.to_string();
            // 载体邮件必须让买家认出是哪件装备的付款请求；名称只取自同一 claim 快照。
            intent.item_name = claim.equipment_name.clone();
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
            // 卖方货款写明是哪件装备的成交结果；不重算金额或政策。
            intent.item_name = claim.equipment_name.clone();
            intent.credits = claim.amount;
        }
        SettlementObligationKind::BuyerRefund => {
            intent.template = "settlement_buyer_refund";
            intent.reference = claim.obligation_id.to_string();
            // 退款写明原装备；金额仍只来自冻结义务。
            intent.item_name = claim.equipment_name.clone();
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
                    insert_payment_deadline(&mut variables, intent);
                    if let Some(line) = &intent.settlement_line {
                        variables.insert("hold_ppm", line.hold_ppm.to_string());
                        variables.insert("adjustment_ppm", line.adjustment_ppm.to_string());
                        variables.insert("winning_bid", line.end_price.value().to_string());
                        // v2 从成交价统一抵扣实分总优惠；旧八变量仍按原含义和原摘要发送。
                        if intent.settlement_mode == Some(SettlementMode::HoldCurveV2) {
                            variables.insert("pricing_mode", "hold_curve_v2".to_owned());
                        }
                        let pool_credit = line.buyer_pool_credit()?;
                        let adjusted_price = line.buyer_amount.checked_add(pool_credit)?;
                        variables.insert("adjusted_price", adjusted_price.to_string());
                        variables.insert("pool_credit", pool_credit.to_string());
                    }
                }
                "settlement_payment_carrier" => {
                    variables.insert("amount", intent.cod.to_string());
                    variables.insert("item_name", intent.item_name.clone());
                    insert_payment_deadline(&mut variables, intent);
                }
                "settlement_equipment_handoff" => {
                    variables.insert("item_name", intent.item_name.clone());
                }
                "settlement_seller_payout" | "settlement_buyer_refund" => {
                    variables.insert("amount", intent.credits.to_string());
                    variables.insert("item_name", intent.item_name.clone());
                }
                _ => return Err(repository_error("结算履约 payload 模板无效")),
            }
            apply_frozen_variable_shape(&mut variables, intent);
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
    Ok(RequestHash::from_legacy_go_json(&activation)?)
}

fn stable_fulfillment_id(namespace: &str, obligation_id: SettlementId) -> Uuid {
    let obligation_uuid = obligation_id.as_uuid();
    legacy_natural_key_uuid(namespace, &[obligation_uuid.as_bytes()])
}

fn valid_payload_line(value: &str, maximum: usize, allow_empty: bool) -> bool {
    (allow_empty && value.is_empty()) || auction_domain::valid_text(value, maximum)
}

/// 付款期限只以 UTC 秒精度写入正文变量；缺失时保持旧载荷，不臆造期限。
fn insert_payment_deadline(
    variables: &mut BTreeMap<&'static str, String>,
    intent: &SettlementDeliveryIntent,
) {
    if let Some(deadline) = intent.payment_deadline_at {
        variables.insert("payment_deadline", crate::services::public::format_deadline(deadline));
    }
}

/// 该模板历史发布过的冻结变量键集合（含价格变量的 v1/v2 与三变量旧形状）。
/// 新变量只允许进入新投递；已在途投递必须按这里的原集合重放并以原摘要对账。
fn known_frozen_variable_sets(template: &str) -> &'static [&'static [&'static str]] {
    const COD_LEGACY: &[&str] = &["amount", "item_name", "reference"];
    const COD_PRICED_V1: &[&str] = &[
        "adjusted_price",
        "adjustment_ppm",
        "amount",
        "hold_ppm",
        "item_name",
        "pool_credit",
        "reference",
        "winning_bid",
    ];
    const COD_PRICED_V2: &[&str] = &[
        "adjusted_price",
        "adjustment_ppm",
        "amount",
        "hold_ppm",
        "item_name",
        "pool_credit",
        "pricing_mode",
        "reference",
        "winning_bid",
    ];
    const CREDIT_LEGACY: &[&str] = &["amount", "reference"];
    const HANDOFF: &[&str] = &["item_name", "reference"];
    match template {
        "settlement_winner_equipment_cod" => &[COD_LEGACY, COD_PRICED_V1, COD_PRICED_V2],
        "settlement_payment_carrier" | "settlement_seller_payout" | "settlement_buyer_refund" => {
            &[CREDIT_LEGACY]
        }
        "settlement_equipment_handoff" => &[HANDOFF],
        _ => &[],
    }
}

/// 命中已知历史键集合时裁剪到该集合；未命中则保留完整新形状，交给完整 canonical 比较拒绝。
/// 未知形状从不被“修正”成可摘要的载荷，避免把损坏快照当成同一投递。
fn apply_frozen_variable_shape(
    variables: &mut BTreeMap<&'static str, String>,
    intent: &SettlementDeliveryIntent,
) {
    let Some(frozen) = intent.frozen_variables.as_ref() else {
        return;
    };
    let known = known_frozen_variable_sets(intent.template)
        .iter()
        .any(|keys| keys.len() == frozen.len() && keys.iter().all(|key| frozen.contains(*key)));
    if !known {
        return;
    }
    variables.retain(|key, _| frozen.contains(*key));
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use auction_domain::{AuctionId, EquipmentId, LotId, SettlementWorld};
    use chrono::Utc;
    use uuid::Uuid;

    use self::super::{
        CanonicalSettlementDelivery, CommandId, SettlementFulfillmentClaim, SettlementId,
        SettlementObligationKind, build_canonical_delivery, build_delivery_intent,
        stable_delivery_id, stable_prepare_command_id,
    };
    use crate::core::RequestHash;
    use crate::ports::settlement::SettlementFulfillmentOperation;
    use crate::ports::settlement::{
        SettlementFulfillmentDeliverySnapshot, SettlementFulfillmentDeliveryStatus,
        SettlementFulfillmentState,
    };

    const OBLIGATION: &str = "3d7e3f2a-4c1e-4a4c-9a02-1c9a5f0d3b21";

    fn claim(kind: SettlementObligationKind) -> SettlementFulfillmentClaim {
        let now = Utc::now();
        SettlementFulfillmentClaim {
            obligation_id: SettlementId::from_uuid(Uuid::parse_str(OBLIGATION).unwrap()),
            auction_id: AuctionId::from_uuid(Uuid::from_u128(2)),
            lot_id: LotId::from_uuid(Uuid::from_u128(3)),
            origin_command_id: CommandId::from_uuid(Uuid::from_u128(4)),
            world: SettlementWorld::Main,
            kind,
            state: SettlementFulfillmentState::Pending,
            operation: SettlementFulfillmentOperation::Send,
            recipient_user_id: Some(Uuid::from_u128(5)),
            recipient: "buyer".to_owned(),
            amount: 1_000,
            carrier_credit: 1,
            equipment_id: EquipmentId::from_uuid(Uuid::from_u128(6)),
            equipment_world: SettlementWorld::Isekai,
            external_equipment_id: "317632875".to_owned(),
            equipment_name: "Radiant Phase Shoes".to_owned(),
            settlement_line: None,
            settlement_mode: None,
            payment_deadline_at: Some(
                chrono::DateTime::parse_from_rfc3339("2026-11-10T12:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            submission_id: None,
            submission_source_kind: Some("mail".to_owned()),
            submission_external_mail_id: None,
            prerequisite_id: None,
            delivery_id: None,
            prepare_command_id: None,
            prepare_request_hash: None,
            activate_command_id: None,
            activate_request_hash: None,
            delivery_snapshot: None,
            claim_token: Uuid::from_u128(7),
            claim_fencing: 1,
            state_version: 1,
            worker_id: "worker".to_owned(),
            claimed_at: now,
            lease_until: now + chrono::Duration::seconds(60),
            retry_count: 0,
        }
    }

    fn snapshot_with_variables(
        claim: &SettlementFulfillmentClaim,
        variables: serde_json::Value,
    ) -> SettlementFulfillmentDeliverySnapshot {
        SettlementFulfillmentDeliverySnapshot {
            id: stable_delivery_id(claim.obligation_id),
            status: SettlementFulfillmentDeliveryStatus::Prepared,
            prepare_command_id: stable_prepare_command_id(claim.obligation_id),
            prepare_request_hash: RequestHash::new([9; 32]),
            activate_command_id: None,
            activate_request_hash: None,
            origin_kind: "settlement".to_owned(),
            origin_id: claim.obligation_id.to_string(),
            recipient: claim.recipient.clone(),
            payload: serde_json::json!({"variables": variables}),
            external_reference: None,
            external_reference_proven: false,
            failure_code: None,
        }
    }

    fn variables_of(delivery: &CanonicalSettlementDelivery) -> BTreeMap<String, String> {
        delivery
            .payload
            .get("variables")
            .and_then(serde_json::Value::as_object)
            .map(|variables| {
                variables
                    .iter()
                    .map(|(key, value)| {
                        (key.clone(), value.as_str().unwrap_or_default().to_owned())
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn carrier_delivery_carries_item_and_payment_deadline() {
        let claim = claim(SettlementObligationKind::WinnerPaymentCarrierCod);
        let intent = build_delivery_intent(&claim).expect("载体 intent 必须有效");
        let canonical = build_canonical_delivery(&intent).expect("载体 payload 必须有效");
        let variables = variables_of(&canonical);
        assert_eq!(variables.get("amount").map(String::as_str), Some("1001"));
        assert_eq!(variables.get("item_name").map(String::as_str), Some("Radiant Phase Shoes"));
        assert_eq!(
            variables.get("payment_deadline").map(String::as_str),
            Some("2026-11-10T12:00:00Z")
        );
        assert_eq!(canonical.payload.get("credits"), Some(&serde_json::json!(1)));
        assert_eq!(canonical.payload.get("cod"), Some(&serde_json::json!(1001)));
    }

    #[test]
    fn payout_and_refund_carry_item_name() {
        for kind in [SettlementObligationKind::SellerPayout, SettlementObligationKind::BuyerRefund]
        {
            let mut claim = claim(kind);
            claim.carrier_credit = 0;
            let intent = build_delivery_intent(&claim).expect("金额 intent 必须有效");
            let canonical = build_canonical_delivery(&intent).expect("金额 payload 必须有效");
            let variables = variables_of(&canonical);
            assert_eq!(variables.get("amount").map(String::as_str), Some("1000"));
            assert_eq!(
                variables.get("item_name").map(String::as_str),
                Some("Radiant Phase Shoes"),
                "{kind:?} 必须写明装备"
            );
            assert!(!variables.contains_key("payment_deadline"));
        }
    }

    #[test]
    fn frozen_legacy_credit_payload_is_reproduced_exactly() {
        let mut claim = claim(SettlementObligationKind::SellerPayout);
        claim.carrier_credit = 0;
        let legacy_variables = serde_json::json!({
            "amount": "1000",
            "reference": claim.obligation_id.to_string(),
        });
        claim.delivery_snapshot = Some(snapshot_with_variables(&claim, legacy_variables));
        let intent = build_delivery_intent(&claim).expect("旧载荷 intent 必须有效");
        let canonical = build_canonical_delivery(&intent).expect("旧 payload 必须有效");
        let variables = variables_of(&canonical);
        // 新变量只进入新投递；在途投递必须保持原键集合与原摘要。
        assert_eq!(variables.len(), 2, "{variables:?}");
        assert_eq!(variables.get("amount").map(String::as_str), Some("1000"));
        assert!(!variables.contains_key("item_name"));
    }

    #[test]
    fn unknown_frozen_shape_is_not_rewritten_into_a_match() {
        let mut claim = claim(SettlementObligationKind::SellerPayout);
        claim.carrier_credit = 0;
        claim.delivery_snapshot = Some(snapshot_with_variables(
            &claim,
            serde_json::json!({
                "amount": "1000",
                "reference": claim.obligation_id.to_string(),
                "note": "legacy",
            }),
        ));
        let intent = build_delivery_intent(&claim).expect("intent 构建不因未知形状放宽");
        let canonical = build_canonical_delivery(&intent).expect("新形状 payload 必须有效");
        let variables = variables_of(&canonical);
        // 未知键集合不被裁剪成“已知历史形状”，完整快照比较仍然失败关闭。
        assert!(variables.contains_key("item_name"));
        assert!(!variables.contains_key("note"));
    }

    #[test]
    fn carrier_delivery_requires_a_known_item() {
        let claim = claim(SettlementObligationKind::WinnerPaymentCarrierCod);
        let mut missing_item = claim.clone();
        missing_item.equipment_name = String::new();
        assert!(build_delivery_intent(&missing_item).is_err());
        let mut intent = build_delivery_intent(&claim).expect("载体 intent 必须有效");
        intent.item_name = String::new();
        assert!(!intent.validate());
    }
}
