use {
    crate::database::recovery::scheduler::map_storage_error,
    crate::database::recovery::scheduler::returns::model::RecoveryReturnRow, sqlx::Row,
};

pub(super) fn map_recovery_return_row(
    row: sqlx::postgres::PgRow,
) -> Result<RecoveryReturnRow, auction_application::core::RepositoryError> {
    Ok(RecoveryReturnRow {
        recovery_id: row.try_get("recovery_id").map_err(map_storage_error)?,
        return_mail_id: row.try_get("return_mail_id").map_err(map_storage_error)?,
        return_external_mail_id: row
            .try_get("return_external_mail_id")
            .map_err(map_storage_error)?,
        return_world_id: row.try_get("return_world_id").map_err(map_storage_error)?,
        return_metadata: row.try_get("return_metadata").map_err(map_storage_error)?,
        return_received_at: row.try_get("return_received_at").map_err(map_storage_error)?,
        returned_obligation_id: row.try_get("returned_obligation_id").map_err(map_storage_error)?,
        returned_obligation_kind: row
            .try_get("returned_obligation_kind")
            .map_err(map_storage_error)?,
        returned_carrier_credit: row
            .try_get("returned_carrier_credit")
            .map_err(map_storage_error)?,
        external_equipment_id: row.try_get("external_equipment_id").map_err(map_storage_error)?,
        recovery_world_id: row.try_get("recovery_world_id").map_err(map_storage_error)?,
        cod_return_count: row.try_get("cod_return_count").map_err(map_storage_error)?,
        cod_return_limit: row.try_get("cod_return_limit").map_err(map_storage_error)?,
        payment_obligation_id: row.try_get("payment_obligation_id").map_err(map_storage_error)?,
        equipment_obligation_id: row
            .try_get("equipment_obligation_id")
            .map_err(map_storage_error)?,
        state_version: row.try_get("state_version").map_err(map_storage_error)?,
        created_at: row.try_get("created_at").map_err(map_storage_error)?,
    })
}
