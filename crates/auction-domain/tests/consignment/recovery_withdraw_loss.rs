//! 直接撤回与灭失分流的领域规则回归：每条规则一正一反。

use auction_domain::consignment::deposit::PositionState;
use auction_domain::consignment::recovery::{
    CurrentGeneration, DirectWithdrawalDecision, DirectWithdrawalFacts, DirectWithdrawalRejection,
    LossRoute, LossRoutingError, LossRoutingFacts, decide_direct_withdrawal, route_position_loss,
};
use uuid::Uuid;

fn withdrawal(generation: CurrentGeneration) -> DirectWithdrawalFacts {
    DirectWithdrawalFacts {
        seller_is_rights_holder: true,
        position_state: PositionState::InCustody,
        generation,
        opposite_unknown_write: false,
        safe_to_return_in_season: true,
        active_hold: false,
    }
}

#[test]
fn unsold_generation_binds_directly_and_sealed_sale_appends_a_withdrawal_only_generation() {
    assert_eq!(
        decide_direct_withdrawal(&withdrawal(CurrentGeneration::Unsold)),
        DirectWithdrawalDecision::BindCurrentGeneration
    );
    let proof = Uuid::from_u128(9);
    assert_eq!(
        decide_direct_withdrawal(&withdrawal(CurrentGeneration::SaleSealed {
            generation: 3,
            predecessor_proof: proof,
        })),
        DirectWithdrawalDecision::AppendWithdrawalOnlyGeneration {
            predecessor: 3,
            predecessor_proof: proof
        }
    );
}

#[test]
fn open_sale_is_equipment_committed() {
    assert_eq!(
        decide_direct_withdrawal(&withdrawal(CurrentGeneration::SaleOpen)),
        DirectWithdrawalDecision::Reject(DirectWithdrawalRejection::EquipmentCommitted)
    );
}

#[test]
fn only_the_rights_holder_in_custody_may_withdraw() {
    let mut facts = withdrawal(CurrentGeneration::Unsold);
    facts.seller_is_rights_holder = false;
    assert_eq!(
        decide_direct_withdrawal(&facts),
        DirectWithdrawalDecision::Reject(DirectWithdrawalRejection::NotRightsHolder)
    );
    for state in PositionState::ALL {
        let mut facts = withdrawal(CurrentGeneration::Unsold);
        facts.position_state = state;
        let expected = if state == PositionState::InCustody {
            DirectWithdrawalDecision::BindCurrentGeneration
        } else {
            DirectWithdrawalDecision::Reject(DirectWithdrawalRejection::CustodyUnproven)
        };
        assert_eq!(decide_direct_withdrawal(&facts), expected, "{state:?}");
    }
}

#[test]
fn unknown_opposite_write_and_closed_season_block_the_withdrawal() {
    let mut facts = withdrawal(CurrentGeneration::Unsold);
    facts.opposite_unknown_write = true;
    assert_eq!(
        decide_direct_withdrawal(&facts),
        DirectWithdrawalDecision::Reject(DirectWithdrawalRejection::CustodyUnproven)
    );
    let mut facts = withdrawal(CurrentGeneration::Unsold);
    facts.safe_to_return_in_season = false;
    assert_eq!(
        decide_direct_withdrawal(&facts),
        DirectWithdrawalDecision::Reject(DirectWithdrawalRejection::SeasonClosed)
    );
}

/// 决定 24：独立运营止售不阻拦取物。
#[test]
fn an_active_operations_hold_does_not_block_the_withdrawal() {
    let mut facts = withdrawal(CurrentGeneration::Unsold);
    facts.active_hold = true;
    assert_eq!(decide_direct_withdrawal(&facts), DirectWithdrawalDecision::BindCurrentGeneration);
}

fn loss(delivery: Option<bool>, funding: Option<bool>) -> LossRoutingFacts {
    LossRoutingFacts {
        proof_complete: true,
        current_generation_buyer_delivery_right: delivery,
        current_generation_funding_liability: funding,
        opposite_unknown_write: false,
    }
}

#[test]
fn loss_closes_the_position_only_when_both_liabilities_are_known_absent() {
    assert_eq!(route_position_loss(&loss(Some(false), Some(false))), Ok(LossRoute::ClosePosition));
    for (delivery, funding) in
        [(Some(true), Some(false)), (Some(false), Some(true)), (Some(true), Some(true))]
    {
        assert_eq!(route_position_loss(&loss(delivery, funding)), Ok(LossRoute::ResolveInSale));
    }
}

#[test]
fn unknown_liability_or_opposite_write_keeps_the_occupancy() {
    for (delivery, funding) in
        [(None, Some(false)), (Some(false), None), (None, None), (Some(true), None)]
    {
        assert_eq!(route_position_loss(&loss(delivery, funding)), Ok(LossRoute::KeepOccupancy));
    }
    let mut facts = loss(Some(false), Some(false));
    facts.opposite_unknown_write = true;
    assert_eq!(route_position_loss(&facts), Ok(LossRoute::KeepOccupancy));
    let mut facts = loss(Some(true), Some(true));
    facts.opposite_unknown_write = true;
    assert_eq!(route_position_loss(&facts), Ok(LossRoute::KeepOccupancy));
}

#[test]
fn incomplete_loss_proof_is_rejected() {
    let mut facts = loss(Some(false), Some(false));
    facts.proof_complete = false;
    assert_eq!(route_position_loss(&facts), Err(LossRoutingError::ProofIncomplete));
}
