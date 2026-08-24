//! 结算履约行映射、枚举转换和错误校验。

use super::*;

#[cfg(feature = "postgres")]
pub(super) fn validate_claim_identity(
    claim: &SettlementFulfillmentClaim,
) -> Result<(), auction_application::RepositoryError> {
    if claim.obligation_id.is_nil()
        || claim.claim_token.is_nil()
        || claim.claim_fencing <= 0
        || claim.state_version == 0
        || claim.worker_id.is_empty()
        || claim.worker_id != claim.worker_id.trim()
    {
        return Err(repository_error_owned("结算履约 claim 身份无效"));
    }
    Ok(())
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
    match value.as_str() {
        "main" => Ok(SettlementWorld::Main),
        "isekai" => Ok(SettlementWorld::Isekai),
        _ => Err(repository_error_owned("结算履约世界标识无效")),
    }
}

#[cfg(feature = "postgres")]
pub(super) fn fulfillment_operation(
    value: String,
) -> Result<SettlementFulfillmentOperation, auction_application::RepositoryError> {
    match value.as_str() {
        "send" => Ok(SettlementFulfillmentOperation::Send),
        "return" => Ok(SettlementFulfillmentOperation::Return),
        _ => Err(repository_error_owned("结算履约操作无效")),
    }
}

#[cfg(feature = "postgres")]
pub(super) fn obligation_kind(
    value: String,
) -> Result<SettlementObligationKind, auction_application::RepositoryError> {
    match value.as_str() {
        "winner_equipment_cod" => Ok(SettlementObligationKind::WinnerEquipmentCod),
        "winner_payment_carrier_cod" => Ok(SettlementObligationKind::WinnerPaymentCarrierCod),
        "winner_equipment_handoff" => Ok(SettlementObligationKind::WinnerEquipmentHandoff),
        "seller_payout" => Ok(SettlementObligationKind::SellerPayout),
        "buyer_refund" => Ok(SettlementObligationKind::BuyerRefund),
        "unsold_return" => Ok(SettlementObligationKind::UnsoldReturn),
        "withdrawn_return" => Ok(SettlementObligationKind::WithdrawnReturn),
        _ => Err(repository_error_owned("结算履约义务种类无效")),
    }
}

pub(super) fn bounded_text(value: String, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(super) fn valid_failure_code(value: &str) -> bool {
    matches!(
        value,
        "invalid_fulfillment_fact"
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
    auction_application::RepositoryError::Failed(match error {
        sqlx::Error::RowNotFound => "结算履约记录不存在".to_owned(),
        _ => "结算履约数据库操作失败".to_owned(),
    })
}
