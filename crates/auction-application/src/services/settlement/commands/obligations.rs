//! 结算义务的闭集构造与稳定身份派生。

use std::collections::HashMap;

use auction_domain::{
    AuctionId, CommandId, SettlementId, SettlementObligation, SettlementObligationKind,
    SettlementObligationState, SettlementPlan, SettlementWorld, legacy_natural_key_uuid,
};

use crate::SettlementSnapshot;

/// 根据已计算的结算明细构造支付、交接、卖家结算和退款义务；不复制或修改输入计划。
pub(super) fn build_obligations(
    command_id: CommandId,
    snapshot: &SettlementSnapshot,
    plan: &SettlementPlan,
) -> Result<Vec<SettlementObligation>, auction_domain::DomainError> {
    let mut obligations = Vec::new();
    for_each_obligation(command_id, snapshot, plan, |obligation| obligations.push(obligation))?;
    Ok(obligations)
}

/// 逐条重建并比较义务，保持自然键、顺序及全部字段校验，不保留第二份完整列表。
pub(super) fn validate_obligations(
    command_id: CommandId,
    snapshot: &SettlementSnapshot,
    plan: &SettlementPlan,
) -> Result<(), auction_domain::DomainError> {
    let mut actual = plan.obligations.iter();
    let mut matches = true;
    for_each_obligation(command_id, snapshot, plan, |expected| {
        matches &= actual.next() == Some(&expected);
    })?;
    if !matches || actual.next().is_some() {
        return Err(auction_domain::DomainError::InvalidSettlementFacts(
            "结算义务与锁内事实或自然键不一致",
        ));
    }
    Ok(())
}

fn for_each_obligation(
    command_id: CommandId,
    snapshot: &SettlementSnapshot,
    plan: &SettlementPlan,
    mut consume: impl FnMut(SettlementObligation),
) -> Result<(), auction_domain::DomainError> {
    // 只索引借用的明细；输出仍严格沿用锁内事实顺序，不能依赖 HashMap 迭代顺序。
    let mut lines_by_id = HashMap::with_capacity(plan.lines.len());
    for line in &plan.lines {
        lines_by_id.entry(line.lot_id).or_insert(line);
    }

    for fact in &snapshot.facts {
        let line = lines_by_id
            .get(&fact.lot_id)
            .ok_or(auction_domain::DomainError::InvalidSettlementFacts("结算明细缺少对应 lot"))?;

        let Some(winner_id) = fact.leader_id else {
            consume(SettlementObligation {
                id: obligation_id(snapshot.auction_id, fact.lot_id, "unsold_return"),
                auction_id: snapshot.auction_id,
                lot_id: fact.lot_id,
                origin_command_id: command_id,
                policy_version: plan.policy_version,
                world: fact.world,
                kind: SettlementObligationKind::UnsoldReturn,
                state: SettlementObligationState::Ready,
                state_version: 1,
                recipient_user_id: None,
                recipient_game_identity: fact.seller_identity.clone(),
                amount: auction_domain::SettlementAmount::zero(),
                carrier_credit: 0,
                equipment_id: fact.equipment_id,
                submission_id: fact.submission_id,
                prerequisite_id: None,
            });
            continue;
        };

        let payment_kind = match fact.world {
            SettlementWorld::Main => SettlementObligationKind::WinnerEquipmentCod,
            SettlementWorld::Isekai => SettlementObligationKind::WinnerPaymentCarrierCod,
        };
        let payment_id = obligation_id(snapshot.auction_id, fact.lot_id, payment_kind.as_str());
        consume(SettlementObligation {
            id: payment_id,
            auction_id: snapshot.auction_id,
            lot_id: fact.lot_id,
            origin_command_id: command_id,
            policy_version: plan.policy_version,
            // carrier COD 的义务事实仍属于异世界；主世界只在履约投影的 dispatch_world 表达。
            world: fact.world,
            kind: payment_kind,
            state: SettlementObligationState::Ready,
            state_version: 1,
            recipient_user_id: Some(winner_id),
            recipient_game_identity: String::new(),
            amount: line.buyer_amount,
            carrier_credit: if fact.world == SettlementWorld::Isekai {
                snapshot.policy.isekai_carrier_credit
            } else {
                0
            },
            equipment_id: fact.equipment_id,
            submission_id: fact.submission_id,
            prerequisite_id: None,
        });

        let seller_prerequisite = if fact.world == SettlementWorld::Isekai {
            let handoff_id =
                obligation_id(snapshot.auction_id, fact.lot_id, "winner_equipment_handoff");
            consume(SettlementObligation {
                id: handoff_id,
                auction_id: snapshot.auction_id,
                lot_id: fact.lot_id,
                origin_command_id: command_id,
                policy_version: plan.policy_version,
                world: fact.world,
                kind: SettlementObligationKind::WinnerEquipmentHandoff,
                state: SettlementObligationState::Blocked,
                state_version: 1,
                recipient_user_id: Some(winner_id),
                recipient_game_identity: String::new(),
                amount: auction_domain::SettlementAmount::zero(),
                carrier_credit: 0,
                equipment_id: fact.equipment_id,
                submission_id: fact.submission_id,
                prerequisite_id: Some(payment_id),
            });
            handoff_id
        } else {
            payment_id
        };

        consume(SettlementObligation {
            id: obligation_id(snapshot.auction_id, fact.lot_id, "seller_payout"),
            auction_id: snapshot.auction_id,
            lot_id: fact.lot_id,
            origin_command_id: command_id,
            policy_version: plan.policy_version,
            world: fact.world,
            kind: SettlementObligationKind::SellerPayout,
            state: SettlementObligationState::Blocked,
            state_version: 1,
            recipient_user_id: None,
            recipient_game_identity: fact.seller_identity.clone(),
            amount: line.seller_amount,
            carrier_credit: 0,
            equipment_id: fact.equipment_id,
            submission_id: fact.submission_id,
            prerequisite_id: Some(seller_prerequisite),
        });

        consume(SettlementObligation {
            id: obligation_id(snapshot.auction_id, fact.lot_id, "buyer_refund"),
            auction_id: snapshot.auction_id,
            lot_id: fact.lot_id,
            origin_command_id: command_id,
            policy_version: plan.policy_version,
            world: fact.world,
            kind: SettlementObligationKind::BuyerRefund,
            state: SettlementObligationState::Contingent,
            state_version: 1,
            recipient_user_id: Some(winner_id),
            recipient_game_identity: String::new(),
            amount: line.buyer_amount,
            carrier_credit: 0,
            equipment_id: fact.equipment_id,
            submission_id: fact.submission_id,
            prerequisite_id: Some(payment_id),
        });
    }

    Ok(())
}

fn obligation_id(auction_id: AuctionId, lot_id: auction_domain::LotId, kind: &str) -> SettlementId {
    let auction_uuid = auction_id.as_uuid();
    let lot_uuid = lot_id.as_uuid();
    SettlementId::from_uuid(legacy_natural_key_uuid(
        "auction.settlement-obligation.v1",
        &[auction_uuid.as_bytes(), lot_uuid.as_bytes(), kind.as_bytes()],
    ))
}
