//! 合同的类型化义务图：固定形状、稳定 ID 派生与重建比较。

use uuid::Uuid;

use super::{
    CONSIGNMENT_SETTLEMENT_POLICY_VERSION, ConsignmentContract, ConsignmentContractError,
    ConsignmentContractKind,
};
use crate::settlement::{ObligationShape, validate_obligation_shape};
use crate::{
    ConsignmentContractId, SettlementAmount, SettlementId, SettlementObligationKind,
    SettlementObligationState, SettlementWorld, UserId, legacy_natural_key_uuid,
};

/// 寄售义务 ID 的派生 namespace；已发布后不可修改，由黄金向量固定。
const OBLIGATION_ID_NAMESPACE: &str = "consignment.settlement-obligation.v1";

/// 寄售合同产生的一条结算义务；与拍卖义务共用形状矩阵，但来源是合同而不是 lot。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsignmentObligation {
    pub id: SettlementId,
    pub contract_id: ConsignmentContractId,
    pub contract_kind: ConsignmentContractKind,
    /// 合同世界；carrier 义务也记 isekai，派发世界由履约行表达。
    pub world: SettlementWorld,
    pub kind: SettlementObligationKind,
    pub state: SettlementObligationState,
    pub state_version: u64,
    pub policy_version: u32,
    /// 买家腿的收款人；卖款与撤回为 `None`。
    pub recipient_user_id: Option<UserId>,
    /// 卖款与撤回为冻结卖家身份，其余为空串。
    pub recipient_game_identity: String,
    pub amount: SettlementAmount,
    pub carrier_credit: u32,
    pub custody_equipment_id: Uuid,
    pub prerequisite_id: Option<SettlementId>,
}

impl ConsignmentObligation {
    /// 单条义务的矩阵校验；整图的形状由 [`validate_consignment_graph`] 重建比较。
    fn validate(&self) -> Result<(), ConsignmentContractError> {
        let has_user = self.recipient_user_id.is_some_and(|user_id| !user_id.is_nil());
        let has_identity = !self.recipient_game_identity.trim().is_empty()
            && self.recipient_game_identity == self.recipient_game_identity.trim();
        if self.id.is_nil()
            || self.contract_id.is_nil()
            || self.state_version == 0
            || self.custody_equipment_id.is_nil()
            || self.prerequisite_id.is_some_and(SettlementId::is_nil)
            || self.recipient_user_id.is_some_and(UserId::is_nil)
            || has_user && !self.recipient_game_identity.is_empty()
        {
            return Err(ConsignmentContractError::Graph("义务身份或收款人不完整"));
        }
        validate_obligation_shape(ObligationShape {
            kind: self.kind,
            world: self.world,
            state: self.state,
            has_user,
            has_identity,
            amount: self.amount,
            carrier_credit: self.carrier_credit,
            prerequisite_present: self.prerequisite_id.is_some(),
        })?;
        Ok(())
    }
}

/// 合同及其全部义务，按固定写入顺序排列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsignmentObligationGraph {
    pub contract: ConsignmentContract,
    pub obligations: Vec<ConsignmentObligation>,
}

/// 以合同 UUID 与 kind 稳定名称派生义务 ID；算法同拍卖义务的 [`legacy_natural_key_uuid`]，
/// namespace 固定，重放同一合同必得同一 ID（不变量 1）。
pub fn consignment_obligation_id(
    contract_id: ConsignmentContractId,
    kind: SettlementObligationKind,
) -> SettlementId {
    let contract_uuid = contract_id.as_uuid();
    SettlementId::from_uuid(legacy_natural_key_uuid(
        OBLIGATION_ID_NAMESPACE,
        &[contract_uuid.as_bytes(), kind.as_str().as_bytes()],
    ))
}

/// 义务的收款人腿。
enum Recipient {
    Buyer(UserId),
    Seller,
}

/// 一条义务除合同外的可变部分，避免构造函数参数过长。
struct Leg {
    kind: SettlementObligationKind,
    state: SettlementObligationState,
    recipient: Recipient,
    amount: i64,
    carrier_credit: u32,
    prerequisite_id: Option<SettlementId>,
}

fn obligation(
    contract: &ConsignmentContract,
    leg: Leg,
) -> Result<ConsignmentObligation, ConsignmentContractError> {
    let core = contract.core();
    let (recipient_user_id, recipient_game_identity) = match leg.recipient {
        Recipient::Buyer(user_id) => (Some(user_id), String::new()),
        Recipient::Seller => (None, core.frozen_seller_identity.clone()),
    };
    let obligation = ConsignmentObligation {
        id: consignment_obligation_id(core.contract_id, leg.kind),
        contract_id: core.contract_id,
        contract_kind: contract.kind(),
        world: core.world,
        kind: leg.kind,
        state: leg.state,
        state_version: 1,
        policy_version: CONSIGNMENT_SETTLEMENT_POLICY_VERSION,
        recipient_user_id,
        recipient_game_identity,
        amount: SettlementAmount::new(leg.amount)?,
        carrier_credit: leg.carrier_credit,
        custody_equipment_id: core.custody_equipment_id,
        prerequisite_id: leg.prerequisite_id,
    };
    obligation.validate()?;
    Ok(obligation)
}

/// 按固定形状构造义务图：main Sale 三条、isekai Sale 四条、Withdrawal 一条。
///
/// 形状与拍卖构图逐项同形，所以既有义务表的 kind 矩阵无需改动；金额只来自合同条款，
/// 撤回合同没有任何金额来源（不变量 6）。
pub fn build_consignment_obligations(
    contract: &ConsignmentContract,
) -> Result<ConsignmentObligationGraph, ConsignmentContractError> {
    contract.validate()?;
    let core = contract.core();
    let mut obligations = Vec::new();
    match contract {
        ConsignmentContract::Sale { terms, .. } => {
            let price = terms.price.value();
            let buyer = || Recipient::Buyer(terms.buyer_user_id);
            let (payment_kind, carrier_credit) = match core.world {
                SettlementWorld::Main => (SettlementObligationKind::WinnerEquipmentCod, 0),
                SettlementWorld::Isekai => (SettlementObligationKind::WinnerPaymentCarrierCod, 1),
            };
            let payment = obligation(
                contract,
                Leg {
                    kind: payment_kind,
                    state: SettlementObligationState::Ready,
                    recipient: buyer(),
                    amount: price,
                    carrier_credit,
                    prerequisite_id: None,
                },
            )?;
            let payment_id = payment.id;
            obligations.push(payment);
            // 卖款的前置：main 是付款腿，isekai 是交接腿。
            let mut payout_prerequisite = payment_id;
            if core.world == SettlementWorld::Isekai {
                let handoff = obligation(
                    contract,
                    Leg {
                        kind: SettlementObligationKind::WinnerEquipmentHandoff,
                        state: SettlementObligationState::Blocked,
                        recipient: buyer(),
                        amount: 0,
                        carrier_credit: 0,
                        prerequisite_id: Some(payment_id),
                    },
                )?;
                payout_prerequisite = handoff.id;
                obligations.push(handoff);
            }
            obligations.push(obligation(
                contract,
                Leg {
                    kind: SettlementObligationKind::SellerPayout,
                    state: SettlementObligationState::Blocked,
                    recipient: Recipient::Seller,
                    amount: terms.quote.seller_amount,
                    carrier_credit: 0,
                    prerequisite_id: Some(payout_prerequisite),
                },
            )?);
            obligations.push(obligation(
                contract,
                Leg {
                    kind: SettlementObligationKind::BuyerRefund,
                    state: SettlementObligationState::Contingent,
                    recipient: buyer(),
                    amount: price,
                    carrier_credit: 0,
                    prerequisite_id: Some(payment_id),
                },
            )?);
        }
        ConsignmentContract::Withdrawal { .. } => {
            obligations.push(obligation(
                contract,
                Leg {
                    kind: SettlementObligationKind::WithdrawnReturn,
                    state: SettlementObligationState::Ready,
                    recipient: Recipient::Seller,
                    amount: 0,
                    carrier_credit: 0,
                    prerequisite_id: None,
                },
            )?);
        }
    }
    Ok(ConsignmentObligationGraph { contract: contract.clone(), obligations })
}

/// 重建并逐项比较（顺序、ID、字段、前置）；任何篡改、缺项、多项或换序都失败关闭。
pub fn validate_consignment_graph(
    graph: &ConsignmentObligationGraph,
) -> Result<(), ConsignmentContractError> {
    let expected = build_consignment_obligations(&graph.contract)?;
    if graph.obligations.len() != expected.obligations.len() {
        return Err(ConsignmentContractError::Graph("义务条数与合同形状不符"));
    }
    for (actual, expected) in graph.obligations.iter().zip(&expected.obligations) {
        actual.validate()?;
        if actual != expected {
            return Err(ConsignmentContractError::Graph("义务与合同重建结果不一致"));
        }
    }
    Ok(())
}
