//! 结算履约行映射、枚举转换和错误校验。

use super::*;

#[cfg(feature = "postgres")]
pub(super) enum ClaimMappingError {
    /// execution 或其 delivery 快照损坏时必须失败关闭并回滚领取事务。
    Execution(auction_application::RepositoryError),
    /// execution 已经有效，但 auction-owned 上下文不满足旧版校验时可隔离。
    Context(auction_application::RepositoryError),
}

#[cfg(feature = "postgres")]
pub(super) struct ClaimExecutionIdentity {
    pub(super) obligation_id: Uuid,
    pub(super) execution_state: String,
}

#[cfg(feature = "postgres")]
pub(super) fn map_claim_execution_identity(
    row: &PgRow,
) -> Result<ClaimExecutionIdentity, auction_application::RepositoryError> {
    let snapshot = auction_application::SettlementClaimExecutionSnapshot {
        obligation_id: uuid_column(row, "obligation_id")?,
        execution_state: string_column(row, "execution_state")?,
        state_version: row.try_get("state_version").map_err(map_storage_error)?,
        claim_fencing: row.try_get("claim_fencing").map_err(map_storage_error)?,
        retry_count: row.try_get("retry_count").map_err(map_storage_error)?,
        dispatch_world: string_column(row, "dispatch_world")?,
        delivery_operation: string_column(row, "delivery_operation")?,
        delivery_id: row.try_get("fulfillment_delivery_id").map_err(map_storage_error)?,
        prepare_command_id: row.try_get("prepare_command_id").map_err(map_storage_error)?,
        prepare_request_digest: row.try_get("prepare_request_digest").map_err(map_storage_error)?,
        activate_command_id: row.try_get("activate_command_id").map_err(map_storage_error)?,
        activate_request_digest: row
            .try_get("activate_request_digest")
            .map_err(map_storage_error)?,
        delivery_snapshot_id: row.try_get("delivery_snapshot_id").map_err(map_storage_error)?,
    };
    snapshot.evaluate().map_err(|blocker| repository_error_owned(blocker.as_str()))?;
    Ok(ClaimExecutionIdentity {
        obligation_id: snapshot.obligation_id,
        execution_state: snapshot.execution_state,
    })
}

#[cfg(feature = "postgres")]
pub(super) fn validate_claim_context(
    row: &PgRow,
) -> Result<(), auction_application::RepositoryError> {
    let snapshot = auction_application::SettlementClaimContextSnapshot {
        obligation_world: string_column(row, "obligation_world")?,
        equipment_world: row.try_get("equipment_world").map_err(map_storage_error)?,
        auction_id: uuid_column(row, "auction_id")?,
        lot_id: uuid_column(row, "lot_id")?,
        origin_command_id: uuid_column(row, "origin_command_id")?,
        equipment_id: uuid_column(row, "equipment_id")?,
        external_equipment_id: row.try_get("external_equipment_id").map_err(map_storage_error)?,
        equipment_name: row.try_get("equipment_name").map_err(map_storage_error)?,
        recipient_user_id: row.try_get("recipient_user_id").map_err(map_storage_error)?,
        recipient_user_game_identity: row
            .try_get("recipient_user_game_identity")
            .map_err(map_storage_error)?,
        recipient_game_identity: row
            .try_get("recipient_game_identity")
            .map_err(map_storage_error)?,
        amount: row.try_get("amount").map_err(map_storage_error)?,
        carrier_credit: row.try_get("carrier_credit").map_err(map_storage_error)?,
        obligation_kind: string_column(row, "obligation_kind")?,
        submission_id: row.try_get("submission_id").map_err(map_storage_error)?,
        submission_source_kind: row.try_get("submission_source_kind").map_err(map_storage_error)?,
        submission_source_reference: row
            .try_get("submission_source_reference")
            .map_err(map_storage_error)?,
        submission_mailbox_key: row.try_get("submission_mailbox_key").map_err(map_storage_error)?,
        submission_external_mail_id: row
            .try_get("submission_external_mail_id")
            .map_err(map_storage_error)?,
        submission_seller: row.try_get("submission_seller").map_err(map_storage_error)?,
        prerequisite_obligation_id: row
            .try_get("prerequisite_obligation_id")
            .map_err(map_storage_error)?,
        prerequisite_row_id: row.try_get("prerequisite_row_id").map_err(map_storage_error)?,
    };
    snapshot.evaluate().map_err(|blocker| repository_error_owned(blocker.as_str()))
}

#[cfg(feature = "postgres")]
pub(super) fn validate_claim_identity(
    claim: &SettlementFulfillmentClaim,
) -> Result<(), auction_application::RepositoryError> {
    claim.validate_identity()
}

#[cfg(feature = "postgres")]
pub(super) fn uuid_column(
    row: &PgRow,
    column: &str,
) -> Result<Uuid, auction_application::RepositoryError> {
    row.try_get(column).map_err(map_storage_error)
}

#[cfg(feature = "postgres")]
pub(super) fn string_column(
    row: &PgRow,
    column: &str,
) -> Result<String, auction_application::RepositoryError> {
    row.try_get(column).map_err(map_storage_error)
}

#[cfg(feature = "postgres")]
pub(super) fn request_hash_from_optional(
    value: Option<Vec<u8>>,
) -> Result<Option<auction_application::RequestHash>, auction_application::RepositoryError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let bytes: [u8; 32] =
        value.try_into().map_err(|_| repository_error_owned("结算履约请求摘要长度无效"))?;
    if bytes == [0; 32] {
        return Err(repository_error_owned("结算履约请求摘要不能为零"));
    }
    Ok(Some(auction_application::RequestHash::new(bytes)))
}

#[cfg(feature = "postgres")]
pub(super) fn settlement_world(
    value: String,
) -> Result<SettlementWorld, auction_application::RepositoryError> {
    SettlementWorld::from_name(&value).ok_or_else(|| repository_error_owned("结算履约世界标识无效"))
}

#[cfg(feature = "postgres")]
pub(super) fn fulfillment_delivery_status(
    value: String,
) -> Result<
    auction_application::SettlementFulfillmentDeliveryStatus,
    auction_application::RepositoryError,
> {
    use auction_application::SettlementFulfillmentDeliveryStatus;

    match value.as_str() {
        "prepared" => Ok(SettlementFulfillmentDeliveryStatus::Prepared),
        "pending" => Ok(SettlementFulfillmentDeliveryStatus::Pending),
        "claimed" => Ok(SettlementFulfillmentDeliveryStatus::Claimed),
        "sending" => Ok(SettlementFulfillmentDeliveryStatus::Sending),
        "succeeded" => Ok(SettlementFulfillmentDeliveryStatus::Succeeded),
        "failed" => Ok(SettlementFulfillmentDeliveryStatus::Failed),
        "manual_reconcile" => Ok(SettlementFulfillmentDeliveryStatus::ManualReconcile),
        _ => Err(repository_error_owned("结算履约 delivery 状态无效")),
    }
}

#[cfg(feature = "postgres")]
pub(super) fn obligation_kind(
    value: String,
) -> Result<SettlementObligationKind, auction_application::RepositoryError> {
    SettlementObligationKind::from_name(&value)
        .ok_or_else(|| repository_error_owned("结算履约义务种类无效"))
}

pub(super) fn bounded_text(value: String, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(super) fn valid_failure_code(value: &str) -> bool {
    matches!(
        value,
        "invalid_fulfillment_fact"
            | "task_panic"
            | "legacy_return_requires_review"
            | "recipient_unavailable"
            | "balance_unavailable"
            | "balance_insufficient"
            | "balance_stale"
            | "delivery_conflict"
            | "delivery_unavailable"
            | "delivery_failed"
            | "stale_fulfillment_claim"
            | "evidence_not_observed"
            | "evidence_unavailable"
            | "evidence_mismatch"
            | "fulfillment_outcome_unknown"
            | "internal_unsupported"
            | "internal_history_conflict"
            | "internal_evidence_wait"
            | "internal_return_unsupported"
            | "internal_funds_wait"
            | "internal_proof_or_funds_wait"
            | "collected_funding_evidence_wait"
            | "collected_funding_wait"
            | "refund_funding_evidence_wait"
            | "refund_funding_wait"
    )
}

pub(super) fn repository_error(message: &str) -> ApplicationError {
    auction_application::RepositoryError::Failed(message.to_owned()).into()
}

#[cfg(feature = "postgres")]
pub(super) fn repository_error_owned(message: &str) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(message.to_owned())
}

#[cfg(feature = "postgres")]
pub(super) fn map_storage_error(error: sqlx::Error) -> auction_application::RepositoryError {
    let context = match &error {
        sqlx::Error::RowNotFound => "结算履约记录不存在",
        _ => "结算履约数据库操作失败",
    };
    auction_application::RepositoryError::Failed(crate::database::sql_error_context(
        &error, context,
    ))
}
