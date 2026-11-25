//! 损失案件的应用投影到 v2 wire DTO；回执必须与请求逐项一致，否则按结果未知处理。

use auction_application::consignment::{
    AdminLossCaseRecord, AdminLossOutcomeRecord, ConsignmentError, DepositOperation,
    LossCaseOutcome, LossCaseOutcomeReceipt, LossCaseState, LossProofReceipt, PositionLossReceipt,
    ReceiptDisposition,
};
use auction_contracts::{
    AdminLossCase, AdminLossOutcome, ConfirmPositionLossRequest, ListingStateV2, ListingVersion,
    LossCaseDisposition, LossCaseOperation, LossCaseOutcomeKind,
    LossCaseOutcomeReceipt as ReceiptDto, LossCaseState as StateDto, LossCaseVersion,
    LossProofDisposition, LossProofOperation, LossProofReceipt as ProofReceiptDto,
    PositionLossDisposition, PositionLossOperation, PositionLossReceipt as LossReceiptDto,
    RecordLossCaseOutcomeRequest,
};
use auction_domain::consignment::ListingState;
use uuid::Uuid;

/// 请求里的安全整数案件版本转为应用命令的无符号版本。
pub(super) fn case_version(value: i64) -> Result<u64, ConsignmentError> {
    u64::try_from(value).map_err(invalid_request)
}

/// 请求结果种类转为应用命令的结果种类。
pub(super) const fn outcome_kind(kind: LossCaseOutcomeKind) -> LossCaseOutcome {
    match kind {
        LossCaseOutcomeKind::Reviewing => LossCaseOutcome::Reviewing,
        LossCaseOutcomeKind::Closed => LossCaseOutcome::Closed,
    }
}

const fn state_dto(state: LossCaseState) -> StateDto {
    match state {
        LossCaseState::Opened => StateDto::Opened,
        LossCaseState::Reviewing => StateDto::Reviewing,
        LossCaseState::Closed => StateDto::Closed,
    }
}

fn version_dto(value: u64) -> Result<LossCaseVersion, ConsignmentError> {
    i64::try_from(value).ok().and_then(LossCaseVersion::new).ok_or(ConsignmentError::Unavailable)
}

/// 案件读模型；返回的 position 必须是请求的 position，其余形状不符一律不可用。
pub(super) fn case_dto(
    record: AdminLossCaseRecord,
    requested: Uuid,
) -> Result<AdminLossCase, ConsignmentError> {
    if record.position_id != requested {
        return Err(ConsignmentError::Unavailable);
    }
    let outcomes = record.outcomes.into_iter().map(outcome_dto).collect::<Result<Vec<_>, _>>()?;
    let case = AdminLossCase {
        position_id: record.position_id,
        case_id: record.case_id,
        loss_fact_id: record.loss_fact_id,
        proof_id: record.proof_id,
        proof_source_kind: record.proof_source_kind,
        proof_digest: hex_digest(&record.proof_digest),
        listing_id: record.listing_id,
        seller_user_id: record.seller_user_id,
        generation: version_dto(record.generation)?,
        case_version: version_dto(record.case_version)?,
        state: state_dto(record.state),
        outcomes,
    };
    // 经 DTO 的序列化校验复核版本连续与状态一致，异常投影不得以成功状态送出。
    serde_json::to_value(&case).map_err(unavailable)?;
    Ok(case)
}

fn outcome_dto(record: AdminLossOutcomeRecord) -> Result<AdminLossOutcome, ConsignmentError> {
    Ok(AdminLossOutcome {
        outcome_id: record.outcome_id,
        case_version: version_dto(record.case_version)?,
        state: state_dto(record.state),
        basis_id: record.basis_id,
        private_note: record.private_note,
        occurred_at: record.occurred_at,
        seller_receipt_id: record.seller_receipt_id,
    })
}

/// 回执须与请求的命令、案件与版本加一一致；不一致说明写结果无法确认，只能原样重试。
pub(super) fn receipt_dto(
    receipt: LossCaseOutcomeReceipt,
    request: &RecordLossCaseOutcomeRequest,
) -> Result<ReceiptDto, ConsignmentError> {
    let expected_version = request.expected_case_version.get().checked_add(1);
    if receipt.command_id.as_uuid() != request.command_id
        || receipt.case_id != request.case_id
        || i64::try_from(receipt.case_version).ok() != expected_version
    {
        return Err(ConsignmentError::OutcomeUnknown);
    }
    let dto = ReceiptDto {
        schema_version: receipt.schema_version,
        operation: LossCaseOperation::RecordLossCaseOutcome,
        disposition: LossCaseDisposition::CaseOutcomeRecorded,
        command_id: request.command_id,
        case_id: receipt.case_id,
        case_version: version_dto(receipt.case_version).map_err(outcome_unknown)?,
        outcome_id: receipt.outcome_id,
        seller_receipt_id: receipt.seller_receipt_id,
    };
    serde_json::to_value(&dto).map_err(outcome_unknown)?;
    Ok(dto)
}

/// 灭失确认回执须回答本请求：命令、position、固定处置与关闭态一致，position 版本越过预期版本；
/// 不一致说明写结果无法确认，只能原样重试。私有说明不进回执。
pub(super) fn confirm_receipt_dto(
    receipt: PositionLossReceipt,
    request: &ConfirmPositionLossRequest,
    position_id: Uuid,
) -> Result<LossReceiptDto, ConsignmentError> {
    if receipt.schema_version != 2
        || receipt.operation != DepositOperation::ConfirmPositionLoss
        || receipt.disposition != ReceiptDisposition::PositionLossRecorded
        || receipt.state != ListingState::Closed
        || receipt.command_id.as_uuid() != request.command_id
        || receipt.position_id != position_id
        || i64::try_from(receipt.position_version)
            .is_ok_and(|version| version <= request.expected_version.get())
    {
        return Err(ConsignmentError::OutcomeUnknown);
    }
    let listing_version = i64::try_from(receipt.listing_version.value())
        .ok()
        .and_then(ListingVersion::new)
        .ok_or(ConsignmentError::OutcomeUnknown)?;
    let dto = LossReceiptDto {
        schema_version: receipt.schema_version,
        operation: PositionLossOperation::ConfirmPositionLoss,
        disposition: PositionLossDisposition::PositionLossRecorded,
        command_id: request.command_id,
        listing_id: receipt.listing_id,
        listing_version,
        state: ListingStateV2::Closed,
        position_id,
        position_version: version_dto(receipt.position_version).map_err(outcome_unknown)?,
        loss_fact_id: receipt.loss_fact_id,
        loss_case_id: receipt.loss_case_id,
        seller_receipt_id: receipt.seller_receipt_id,
    };
    serde_json::to_value(&dto).map_err(outcome_unknown)?;
    Ok(dto)
}

/// 灭失证据回执须回答本请求：固定版本、操作与处置，命令与 position 一致；不一致说明写结果无法确认，
/// 只能原样重试。私有说明与证据原文不进回执。
pub(super) fn proof_receipt_dto(
    receipt: LossProofReceipt,
    command_id: Uuid,
    position_id: Uuid,
) -> Result<ProofReceiptDto, ConsignmentError> {
    if receipt.schema_version != 2
        || receipt.operation != DepositOperation::RecordPositionLossProof
        || receipt.disposition != ReceiptDisposition::LossProofRecorded
        || receipt.command_id.as_uuid() != command_id
        || receipt.position_id != position_id
    {
        return Err(ConsignmentError::OutcomeUnknown);
    }
    let dto = ProofReceiptDto {
        schema_version: receipt.schema_version,
        operation: LossProofOperation::RecordPositionLossProof,
        disposition: LossProofDisposition::LossProofRecorded,
        command_id,
        position_id,
        proof_id: receipt.proof_id,
        proof_digest: hex_digest(&receipt.proof_digest),
    };
    serde_json::to_value(&dto).map_err(outcome_unknown)?;
    Ok(dto)
}

fn invalid_request<T>(_error: T) -> ConsignmentError {
    ConsignmentError::InvalidDepositRequest
}

fn unavailable<T>(_error: T) -> ConsignmentError {
    ConsignmentError::Unavailable
}

fn outcome_unknown<T>(_error: T) -> ConsignmentError {
    ConsignmentError::OutcomeUnknown
}

fn hex_digest(digest: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(64);
    for byte in digest {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    text
}
