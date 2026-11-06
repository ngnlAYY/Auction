//! 履约 claim 与 delivery 快照、意图的一致性判定。

use super::intent::{build_canonical_delivery, stable_delivery_id, stable_prepare_command_id};
use super::model::SettlementDeliveryIntent;
use crate::ports::settlement::SettlementFulfillmentClaim;

pub(super) fn delivery_snapshot_matches(
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
