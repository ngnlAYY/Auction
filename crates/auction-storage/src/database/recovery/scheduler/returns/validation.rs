//! Recovery 退回的冻结预期；证据对象的形状校验由应用层共享的严格模型负责。

use auction_domain::valid_text as valid_recovery_text;

use crate::database::recovery::scheduler::returns::model::{
    ExpectedRecoveryReturn, RecoveryReturnRow,
};

pub(super) fn expected_recovery_return(row: &RecoveryReturnRow) -> Option<ExpectedRecoveryReturn> {
    match (
        row.recovery_world_id.as_str(),
        row.return_world_id.as_str(),
        row.returned_obligation_id,
        row.returned_obligation_kind.as_str(),
    ) {
        ("main", "main", obligation_id, "winner_equipment_cod")
            if obligation_id == row.payment_obligation_id
                && valid_recovery_text(&row.external_equipment_id, 128) =>
        {
            Some(ExpectedRecoveryReturn {
                kind: "reclaim_equipment_take",
                expected_credits: 0,
                expected_cod: 0,
                expected_attachment_ids: vec![row.external_equipment_id.clone()],
            })
        }
        ("isekai", "main", obligation_id, "winner_payment_carrier_cod")
            if obligation_id == row.payment_obligation_id && row.returned_carrier_credit > 0 =>
        {
            Some(ExpectedRecoveryReturn {
                kind: "cancel_carrier_take",
                expected_credits: row.returned_carrier_credit,
                expected_cod: 0,
                expected_attachment_ids: Vec::new(),
            })
        }
        ("isekai", "isekai", obligation_id, "winner_equipment_handoff")
            if row.equipment_obligation_id == Some(obligation_id)
                && valid_recovery_text(&row.external_equipment_id, 128) =>
        {
            Some(ExpectedRecoveryReturn {
                kind: "recover_equipment_take",
                expected_credits: 0,
                expected_cod: 0,
                expected_attachment_ids: vec![row.external_equipment_id.clone()],
            })
        }
        _ => None,
    }
}
