//! 寄售原信、观察、隔离、终局与外写事实的 PostgreSQL owner 分组。
//!
//! 所有写入沿用 4K 的同一顶层事务与锁序；本模块不自行开事务、不使用保存点。

pub(crate) mod admin_read;
pub(crate) mod disposition;
pub(crate) mod identity;
pub(crate) mod observations;
pub(crate) mod owner;
pub(crate) mod proofs;
pub(crate) mod quarantine;
pub(crate) mod readiness;
pub(crate) mod reads;
pub(crate) mod recheck;
pub(crate) mod release;
pub(crate) mod release_legacy;

use std::fmt::Display;

use auction_application::consignment::{ConsignmentError, DepositBusinessRejection};
use auction_domain::consignment::deposit::DepositError;
use serde_json::Value;

use crate::database::consignment::commands::DepositReceipt;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::StoredReceipt;

/// 同一事务内已落库的成功回执或确定拒绝。
pub(crate) enum Outcome<T> {
    /// 成功回执。
    Accepted(T),
    /// 已写入安全拒绝回执。
    Rejected(DepositBusinessRejection),
}

impl<T> Outcome<T> {
    /// 提交后转换成端口结果；拒绝在提交后才变成 `Err`。
    pub(crate) fn into_result(self) -> Result<T, ConsignmentError> {
        match self {
            Self::Accepted(value) => Ok(value),
            Self::Rejected(rejection) => Err(ConsignmentError::Deposit(rejection)),
        }
    }
}

/// 解码已有 v2 行；拒绝回执只恢复安全码。
pub(crate) fn replay_receipt<T: DepositReceipt>(
    stored: &StoredReceipt,
) -> Result<Outcome<T>, WriteError> {
    if stored.body.get("disposition").and_then(Value::as_str) == Some("rejected") {
        let code = stored.body.get("code").and_then(Value::as_str).unwrap_or_default();
        let rejection =
            decode_rejection(code).ok_or_else(|| invariant("consignment_rejection_code"))?;
        return Ok(Outcome::Rejected(rejection));
    }
    Ok(Outcome::Accepted(T::decode(stored)?))
}

/// 内部校验失败；原因只允许固定机器码。
pub(crate) fn invariant(reason: &'static str) -> WriteError {
    WriteError::Invariant { sqlstate: "none".into(), constraint: reason.into() }
}

/// 解码持久事实失败：记录来源后返回固定内部校验错误。
pub(crate) fn invariant_decoding<E: Display>(reason: &'static str) -> impl FnOnce(E) -> WriteError {
    move |error| {
        tracing::warn!(constraint = reason, error = %error, "寄售持久事实解码失败");
        invariant(reason)
    }
}

/// 读模型解码失败：记录来源后返回能力不可用，不向调用方泄露细节。
pub(crate) fn unavailable_decoding<E: Display>(
    reason: &'static str,
) -> impl FnOnce(E) -> ConsignmentError {
    move |error| {
        tracing::warn!(constraint = reason, error = %error, "寄售读模型解码失败");
        ConsignmentError::Unavailable
    }
}

/// 确定业务拒绝转成事务内错误。
pub(crate) fn conflict(rejection: DepositBusinessRejection) -> WriteError {
    ConsignmentError::Deposit(rejection).into()
}

/// 把持久拒绝码映射回 4A 闭集。
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
        _ => return None,
    })
}

/// 领域拒绝码映射到 v2 确定拒绝闭集。
pub(crate) fn domain_rejection(error: DepositError) -> DepositBusinessRejection {
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
        DepositError::AcquireReturnUnavailable => DepositBusinessRejection::EvidenceUnproven,
        DepositError::EvidenceConflict => DepositBusinessRejection::EvidenceConflict,
        DepositError::DispositionConflict => DepositBusinessRejection::DispositionConflict,
        DepositError::HoldMismatch => DepositBusinessRejection::HoldMismatch,
        DepositError::FenceMismatch => DepositBusinessRejection::FenceMismatch,
        _ => DepositBusinessRejection::ActionNotAllowed,
    }
}
