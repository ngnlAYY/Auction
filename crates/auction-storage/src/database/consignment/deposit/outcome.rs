//! v2 确定拒绝的持久化与端口返回值转换。

use auction_application::consignment::{ConsignmentError, DepositBusinessRejection};
use auction_domain::consignment::deposit::DepositError;
use serde_json::Value;

use crate::database::consignment::commands::DepositReceipt;
use crate::database::consignment::commands::DepositRejection;
use crate::database::consignment::commands::DepositWriter;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::StoredReceipt;

/// 同一事务内已落库的成功回执或确定拒绝。
pub(super) enum PortOutcome<T> {
    /// 成功回执。
    Accepted(T),
    /// 已写入安全拒绝回执。
    Rejected(DepositBusinessRejection),
}

/// 首次执行在写入回执后留给同一事务继续写的子事实。
pub(super) enum FirstOutcome<T, C> {
    /// 回执已落库，按返回的子事实继续写；任一失败整体回滚。
    Accepted {
        /// 成功回执。
        receipt: T,
        /// 仍需写入的预约、控制或通知事实。
        children: C,
    },
    /// 已写入确定拒绝回执。
    Rejected(DepositBusinessRejection),
}

impl<T> PortOutcome<T> {
    /// 提交后转换成端口结果；拒绝在这里才变成 `Err`，保证回执不会随回滚丢失。
    pub(super) fn into_result(self) -> Result<T, ConsignmentError> {
        match self {
            Self::Accepted(value) => Ok(value),
            Self::Rejected(rejection) => Err(ConsignmentError::Deposit(rejection)),
        }
    }
}

/// 在同一事务内结束确定拒绝，回执随事务提交后才在端口层变成错误。
pub(super) async fn reject<T, C>(
    writer: DepositWriter<'_, '_, '_>,
    rejection: DepositBusinessRejection,
    now: auction_time::Timestamp,
) -> Result<FirstOutcome<T, C>, WriteError> {
    writer.finish_rejection(DepositRejection::Business(rejection), now).await?;
    Ok(FirstOutcome::Rejected(rejection))
}

/// 解码已有 v2 行；拒绝回执只恢复安全码，不伪造业务事件。
pub(super) fn replay<T: DepositReceipt>(
    stored: &StoredReceipt,
) -> Result<PortOutcome<T>, WriteError> {
    if stored.body.get("disposition").and_then(Value::as_str) == Some("rejected") {
        let code = stored.body.get("code").and_then(Value::as_str).unwrap_or_default();
        let rejection = decode_rejection(code)
            .ok_or_else(|| WriteError::invariant("consignment_deposit_rejection_code"))?;
        return Ok(PortOutcome::Rejected(rejection));
    }
    Ok(PortOutcome::Accepted(T::decode(stored)?))
}

/// 把持久拒绝码映射回 4A 的确定拒绝闭集。
pub(crate) fn decode_rejection(code: &str) -> Option<DepositBusinessRejection> {
    Some(match code {
        "stale_version" => DepositBusinessRejection::StaleVersion,
        "listing_not_active" => DepositBusinessRejection::ListingNotActive,
        "equipment_busy" => DepositBusinessRejection::EquipmentBusy,
        "participant_ineligible" => DepositBusinessRejection::ParticipantIneligible,
        "bot_party" => DepositBusinessRejection::BotParty,
        "season_closed" => DepositBusinessRejection::SeasonClosed,
        "mail_unavailable" => DepositBusinessRejection::MailUnavailable,
        "source_mismatch" => DepositBusinessRejection::SourceMismatch,
        "evidence_unproven" => DepositBusinessRejection::EvidenceUnproven,
        "evidence_conflict" => DepositBusinessRejection::EvidenceConflict,
        "disposition_conflict" => DepositBusinessRejection::DispositionConflict,
        "hold_mismatch" => DepositBusinessRejection::HoldMismatch,
        "action_not_allowed" => DepositBusinessRejection::ActionNotAllowed,
        "fence_mismatch" => DepositBusinessRejection::FenceMismatch,
        "equipment_committed" => DepositBusinessRejection::EquipmentCommitted,
        "custody_unproven" => DepositBusinessRejection::CustodyUnproven,
        "operations_hold" => DepositBusinessRejection::OperationsHold,
        "listing_limit_reached" => DepositBusinessRejection::ListingLimitReached,
        "opposite_payment_open" => DepositBusinessRejection::OppositePaymentOpen,
        _ => return None,
    })
}

/// 确定业务拒绝转成事务内错误。
pub(super) fn conflict(error: DepositBusinessRejection) -> WriteError {
    ConsignmentError::Deposit(error).into()
}

/// 领域拒绝码映射到 v2 确定拒绝闭集。
pub(super) fn domain(error: DepositError) -> DepositBusinessRejection {
    match error {
        DepositError::StaleVersion { .. } => DepositBusinessRejection::StaleVersion,
        DepositError::ListingNotActive | DepositError::ReservationNotActive => {
            DepositBusinessRejection::ListingNotActive
        }
        DepositError::SellerIneligible => DepositBusinessRejection::ParticipantIneligible,
        DepositError::BotParty => DepositBusinessRejection::BotParty,
        DepositError::SeasonClosed | DepositError::SeasonUnknown => {
            DepositBusinessRejection::SeasonClosed
        }
        DepositError::MailUnavailable => DepositBusinessRejection::MailUnavailable,
        DepositError::EquipmentBusy => DepositBusinessRejection::EquipmentBusy,
        DepositError::SourceMismatch => DepositBusinessRejection::SourceMismatch,
        DepositError::EvidenceUnproven | DepositError::InvalidProof => {
            DepositBusinessRejection::EvidenceUnproven
        }
        DepositError::EvidenceConflict => DepositBusinessRejection::EvidenceConflict,
        DepositError::DispositionConflict => DepositBusinessRejection::DispositionConflict,
        DepositError::HoldMismatch => DepositBusinessRejection::HoldMismatch,
        DepositError::FenceMismatch => DepositBusinessRejection::FenceMismatch,
        _ => DepositBusinessRejection::ActionNotAllowed,
    }
}

/// 领域拒绝直接转成事务内错误。
pub(super) fn domain_write(error: DepositError) -> WriteError {
    conflict(domain(error))
}
