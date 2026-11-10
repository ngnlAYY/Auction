use {
    crate::database::recovery::scheduler::returns::model::{
        ExpectedRecoveryReturn, RecoveryReturnMetadata, RecoveryReturnRow,
    },
    crate::database::recovery::scheduler::{repository_error, valid_recovery_text},
    uuid::Uuid,
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

pub(super) fn validate_recovery_return_metadata(
    metadata: &RecoveryReturnMetadata,
) -> Result<(), auction_application::core::RepositoryError> {
    let obligation_id = Uuid::parse_str(&metadata.obligation_id)
        .map_err(|_| repository_error("recovery 退回邮件 obligation ID 无效"))?;
    if obligation_id.is_nil()
        || obligation_id.to_string() != metadata.obligation_id
        || metadata.credits < 0
        || metadata.cod < 0
        || metadata.attachment_ids.len() > 100
        || metadata.attachment_ids.iter().enumerate().any(|(index, value)| {
            !valid_recovery_text(value, 128)
                || index > 0 && value <= &metadata.attachment_ids[index - 1]
        })
    {
        return Err(repository_error("recovery 退回邮件 metadata 越界或未排序"));
    }
    Ok(())
}
