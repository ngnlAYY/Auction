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
    let obligation_id = uuid_column(row, "obligation_id")?;
    if obligation_id.is_nil() {
        return Err(repository_error_owned("结算履约 execution 缺少 obligation identity"));
    }
    let execution_state = string_column(row, "execution_state")?;
    if !matches!(execution_state.as_str(), "pending" | "prepared" | "enqueued") {
        return Err(repository_error_owned("结算履约 execution 阶段无效"));
    }
    let state_version: i64 = row.try_get("state_version").map_err(map_storage_error)?;
    let claim_fencing: i64 = row.try_get("claim_fencing").map_err(map_storage_error)?;
    let retry_count: i32 = row.try_get("retry_count").map_err(map_storage_error)?;
    if state_version <= 0 || claim_fencing < 0 || retry_count < 0 {
        return Err(repository_error_owned("结算履约 execution 版本或计数器无效"));
    }
    settlement_world(string_column(row, "dispatch_world")?)?;
    fulfillment_operation(string_column(row, "delivery_operation")?)?;

    let delivery_id: Option<Uuid> =
        row.try_get("fulfillment_delivery_id").map_err(map_storage_error)?;
    let prepare_command_id: Option<Uuid> =
        row.try_get("prepare_command_id").map_err(map_storage_error)?;
    let prepare_digest: Option<Vec<u8>> =
        row.try_get("prepare_request_digest").map_err(map_storage_error)?;
    let activate_command_id: Option<Uuid> =
        row.try_get("activate_command_id").map_err(map_storage_error)?;
    let activate_digest: Option<Vec<u8>> =
        row.try_get("activate_request_digest").map_err(map_storage_error)?;
    let has_prepare =
        delivery_id.is_some() || prepare_command_id.is_some() || prepare_digest.is_some();
    if has_prepare
        && (delivery_id.is_none() || prepare_command_id.is_none() || prepare_digest.is_none())
    {
        return Err(repository_error_owned("结算履约 execution prepared identity 不完整"));
    }
    let has_activation = activate_command_id.is_some() || activate_digest.is_some();
    if has_activation
        && (activate_command_id.is_none() || activate_digest.is_none() || !has_prepare)
    {
        return Err(repository_error_owned("结算履约 execution activation identity 不完整"));
    }
    let delivery_snapshot_id: Option<Uuid> =
        row.try_get("delivery_snapshot_id").map_err(map_storage_error)?;
    let expected_shape = match execution_state.as_str() {
        "pending" => !has_prepare && !has_activation && delivery_snapshot_id.is_none(),
        "prepared" => has_prepare && !has_activation && delivery_snapshot_id.is_some(),
        "enqueued" => has_prepare && has_activation && delivery_snapshot_id.is_some(),
        _ => false,
    };
    if !expected_shape {
        return Err(repository_error_owned("结算履约 execution 阶段字段不一致"));
    }
    Ok(ClaimExecutionIdentity { obligation_id, execution_state })
}

#[cfg(feature = "postgres")]
pub(super) fn validate_claim_context(
    row: &PgRow,
) -> Result<(), auction_application::RepositoryError> {
    let obligation_world = string_column(row, "obligation_world")?;
    let equipment_world: Option<String> =
        row.try_get("equipment_world").map_err(map_storage_error)?;
    if !matches!(obligation_world.as_str(), "main" | "isekai")
        || equipment_world.as_deref() != Some(obligation_world.as_str())
    {
        return Err(repository_error_owned("结算履约上下文 world 不一致"));
    }
    let auction_id = uuid_column(row, "auction_id")?;
    let lot_id = uuid_column(row, "lot_id")?;
    let origin_command_id = uuid_column(row, "origin_command_id")?;
    let equipment_id = uuid_column(row, "equipment_id")?;
    if auction_id.is_nil() || lot_id.is_nil() || origin_command_id.is_nil() || equipment_id.is_nil()
    {
        return Err(repository_error_owned("结算履约上下文 identity 无效"));
    }
    let external_equipment_id = string_column(row, "external_equipment_id")?;
    let equipment_name = string_column(row, "equipment_name")?;
    if external_equipment_id.is_empty()
        || external_equipment_id != external_equipment_id.trim()
        || equipment_name.is_empty()
        || equipment_name != equipment_name.trim()
    {
        return Err(repository_error_owned("结算履约上下文 equipment fact 无效"));
    }
    let recipient_user_id: Option<Uuid> =
        row.try_get("recipient_user_id").map_err(map_storage_error)?;
    let recipient_user_game_identity: Option<String> =
        row.try_get("recipient_user_game_identity").map_err(map_storage_error)?;
    let recipient_game_identity: Option<String> =
        row.try_get("recipient_game_identity").map_err(map_storage_error)?;
    let has_user = recipient_user_id.is_some_and(|id| !id.is_nil());
    let has_game_identity = recipient_game_identity
        .as_deref()
        .is_some_and(|value| !value.is_empty() && value == value.trim());
    if has_user == has_game_identity
        || recipient_user_id.is_some_and(|id| id.is_nil())
        || recipient_user_id.is_some() && recipient_user_game_identity.is_none()
        || recipient_user_id.is_none() && recipient_user_game_identity.is_some()
    {
        return Err(repository_error_owned("结算履约上下文收件人 owner 不一致"));
    }
    let amount: i64 = row.try_get("amount").map_err(map_storage_error)?;
    let carrier_credit: i64 = row.try_get("carrier_credit").map_err(map_storage_error)?;
    if amount < 0 || carrier_credit < 0 || carrier_credit > i64::from(u32::MAX) {
        return Err(repository_error_owned("结算履约上下文金额字段无效"));
    }

    let kind = obligation_kind(string_column(row, "obligation_kind")?)?;
    if matches!(
        kind,
        SettlementObligationKind::UnsoldReturn | SettlementObligationKind::WithdrawnReturn
    ) {
        let submission_id: Option<Uuid> =
            row.try_get("submission_id").map_err(map_storage_error)?;
        let source_kind: Option<String> =
            row.try_get("submission_source_kind").map_err(map_storage_error)?;
        let source_reference: Option<String> =
            row.try_get("submission_source_reference").map_err(map_storage_error)?;
        let mailbox_key: Option<String> =
            row.try_get("submission_mailbox_key").map_err(map_storage_error)?;
        let external_mail_id: Option<String> =
            row.try_get("submission_external_mail_id").map_err(map_storage_error)?;
        let seller: Option<String> = row.try_get("submission_seller").map_err(map_storage_error)?;
        if submission_id.is_none()
            || source_kind.is_none()
            || seller.as_deref() != recipient_game_identity.as_deref()
            || seller.as_deref().is_none_or(|value| value.is_empty() || value != value.trim())
        {
            return Err(repository_error_owned("结算履约返还上下文缺少投稿事实"));
        }
        match source_kind.as_deref() {
            Some("mail")
                if source_reference.is_none()
                    && mailbox_key.as_deref()
                        == Some(format!("mooglemail:{obligation_world}").as_str())
                    && external_mail_id
                        .as_deref()
                        .is_some_and(|value| !value.is_empty() && value == value.trim()) => {}
            Some("admin")
                if source_reference
                    .as_deref()
                    .is_some_and(|value| !value.is_empty() && value == value.trim())
                    && mailbox_key.is_none()
                    && external_mail_id.is_none() => {}
            _ => return Err(repository_error_owned("结算履约返还投稿来源无效")),
        }
        if recipient_game_identity.as_deref().is_none_or(|value| value.is_empty()) {
            return Err(repository_error_owned("结算履约返还收件人身份无效"));
        }
    }
    Ok(())
}

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
