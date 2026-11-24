//! Admin 隔离详情、观察与回执的应用投影到 v2 wire DTO。

use auction_application::consignment::{
    AdminDepositAction, AdminDepositDetail, AdminFinalDecision, AdminObservationItem,
    AdminQuarantineObservation, ConsignmentError, ListingStateChange, OperationsHoldReceipt,
    QuarantineState as DomainQuarantineState, ReceiptDisposition, RecheckReceipt, ReleaseReceipt,
};
use auction_contracts::{
    ConsignmentDelistReceiptV2, ConsignmentDispositionV2, ConsignmentHoldReceipt,
    ConsignmentOperationV2, ListingStampV2, ListingStateV2, ListingWorld, QuarantineAction,
    QuarantineDecisionKind, QuarantineDetail, QuarantineObservation, QuarantineObservationItem,
    QuarantineObservationItemKind, QuarantineReason, QuarantineRecheckReceipt,
    QuarantineReleaseOutcome, QuarantineReleaseReceipt, QuarantineReleaseRecord, QuarantineState,
    QuarantineVersion,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::ListingState;
use auction_domain::consignment::deposit::EnvelopeItemKind;
use auction_domain::consignment::deposit::QuarantineReason as DomainQuarantineReason;
use auction_domain::consignment::deposit::ReleaseKind;
use uuid::Uuid;

use super::super::input;

pub(super) fn detail_dto(
    detail: AdminDepositDetail,
    requested: Uuid,
) -> Result<QuarantineDetail, ConsignmentError> {
    if detail.quarantine_id != requested {
        return Err(ConsignmentError::Unavailable);
    }
    let observations =
        detail.observations.into_iter().map(observation_dto).collect::<Result<Vec<_>, _>>()?;
    if observations.is_empty() {
        return Err(ConsignmentError::Unavailable);
    }
    let observation_total = i64::try_from(detail.observation_total).map_err(unavailable)?;
    if observation_total < i64::try_from(observations.len()).map_err(unavailable)? {
        return Err(ConsignmentError::Unavailable);
    }
    let version = i64::try_from(detail.version)
        .ok()
        .and_then(QuarantineVersion::new)
        .ok_or(ConsignmentError::Unavailable)?;
    let release = match (detail.state, detail.final_decision) {
        (DomainQuarantineState::Active, None) => None,
        (DomainQuarantineState::Active, Some(_)) => return Err(ConsignmentError::Unavailable),
        (DomainQuarantineState::Released, Some(final_decision)) => {
            Some(release_record_dto(final_decision)?)
        }
        (DomainQuarantineState::Released, None) => return Err(ConsignmentError::Unavailable),
    };
    Ok(QuarantineDetail {
        quarantine_id: detail.quarantine_id,
        version,
        state: match detail.state {
            DomainQuarantineState::Active => QuarantineState::Active,
            DomainQuarantineState::Released => QuarantineState::Released,
        },
        world: wire_world(detail.world),
        external_mid: detail.external_mid,
        first_decision_id: detail.first_decision_id,
        reason: reason_dto(detail.reason),
        observation_total,
        observations,
        allowed_actions: detail.allowed_actions.into_iter().map(action_dto).collect(),
        release,
    })
}

fn observation_dto(
    observation: AdminQuarantineObservation,
) -> Result<QuarantineObservation, ConsignmentError> {
    let items = observation.items.into_iter().map(item_dto).collect::<Result<Vec<_>, _>>()?;
    let attachment_count = i64::try_from(items.len()).map_err(unavailable)?;
    Ok(QuarantineObservation {
        observation_id: observation.observation_id,
        observed_at: observation.observed_at,
        parser_version: observation.parser_version,
        digest: hex_digest(&observation.digest),
        attachment_count,
        credits: observation.credits,
        cod: observation.cod,
        returnable: observation.returnable,
        takeable: observation.takeable,
        items,
    })
}

fn item_dto(item: AdminObservationItem) -> Result<QuarantineObservationItem, ConsignmentError> {
    let ordinal = i64::from(item.ordinal);
    let (kind, external_equipment_id, amount) = match item.kind {
        EnvelopeItemKind::Equipment => {
            let (Some(eid), None) = (item.canonical_eid, item.amount) else {
                return Err(ConsignmentError::Unavailable);
            };
            (QuarantineObservationItemKind::Equipment, Some(eid), None)
        }
        EnvelopeItemKind::Credits => {
            let (None, Some(amount)) = (item.canonical_eid, item.amount) else {
                return Err(ConsignmentError::Unavailable);
            };
            (QuarantineObservationItemKind::Credits, None, Some(amount))
        }
        EnvelopeItemKind::Cod => {
            let (None, Some(amount)) = (item.canonical_eid, item.amount) else {
                return Err(ConsignmentError::Unavailable);
            };
            (QuarantineObservationItemKind::Cod, None, Some(amount))
        }
        EnvelopeItemKind::Unknown => {
            if item.canonical_eid.is_some() || item.amount.is_some() {
                return Err(ConsignmentError::Unavailable);
            }
            (QuarantineObservationItemKind::Unknown, None, None)
        }
    };
    Ok(QuarantineObservationItem { ordinal, kind, external_equipment_id, amount })
}

fn release_record_dto(
    final_decision: AdminFinalDecision,
) -> Result<QuarantineReleaseRecord, ConsignmentError> {
    if final_decision.release_id.is_nil() || final_decision.decision_id.is_nil() {
        return Err(ConsignmentError::Unavailable);
    }
    let outcome = match final_decision.kind {
        ReleaseKind::Accept => {
            if final_decision.listing_changes.is_empty()
                || final_decision.listing_changes.len() > 20
                || final_decision.obligation_id.is_some()
                || final_decision.return_mail_id.is_some()
                || final_decision.return_evidence_id.is_some()
                || final_decision.recovery_id.is_some()
                || final_decision.recovery_handoff_id.is_some()
                || final_decision.return_intake_id.is_some()
                || final_decision.original_round.is_some()
                || final_decision.take_work_id.is_some()
                || final_decision.take_delivery_id.is_some()
            {
                return Err(ConsignmentError::Unavailable);
            }
            QuarantineReleaseOutcome::Accept {
                listing_changes: final_decision
                    .listing_changes
                    .into_iter()
                    .map(stamp_dto)
                    .collect::<Result<_, _>>()?,
            }
        }
        ReleaseKind::Return | ReleaseKind::ArchiveEmpty => {
            if !final_decision.listing_changes.is_empty()
                || final_decision.obligation_id.is_some()
                || final_decision.return_mail_id.is_some()
                || final_decision.return_evidence_id.is_some()
                || final_decision.recovery_id.is_some()
                || final_decision.recovery_handoff_id.is_some()
                || final_decision.return_intake_id.is_some()
                || final_decision.original_round.is_some()
                || final_decision.take_work_id.is_some()
                || final_decision.take_delivery_id.is_some()
            {
                return Err(ConsignmentError::Unavailable);
            }
            if final_decision.kind == ReleaseKind::Return {
                QuarantineReleaseOutcome::Return {}
            } else {
                QuarantineReleaseOutcome::ArchiveEmpty {}
            }
        }
        ReleaseKind::AcquireReturn => {
            if !final_decision.listing_changes.is_empty()
                || final_decision.obligation_id.is_some()
                || final_decision.return_mail_id.is_some()
                || final_decision.return_evidence_id.is_some()
                || final_decision.recovery_id.is_some()
                || final_decision.recovery_handoff_id.is_some()
            {
                return Err(ConsignmentError::Unavailable);
            }
            let return_intake_id =
                final_decision.return_intake_id.ok_or(ConsignmentError::Unavailable)?;
            let original_round =
                final_decision.original_round.ok_or(ConsignmentError::Unavailable)?;
            let take_work_id = final_decision.take_work_id.ok_or(ConsignmentError::Unavailable)?;
            let take_delivery_id =
                final_decision.take_delivery_id.ok_or(ConsignmentError::Unavailable)?;
            if return_intake_id.is_nil()
                || take_work_id.is_nil()
                || take_delivery_id.is_nil()
                || original_round == 0
            {
                return Err(ConsignmentError::Unavailable);
            }
            QuarantineReleaseOutcome::AcquireReturn {
                return_intake_id,
                original_round: i64::from(original_round),
                take_work_id,
                take_delivery_id,
            }
        }
        ReleaseKind::AuctionReturnCase => {
            if !final_decision.listing_changes.is_empty()
                || final_decision.recovery_id.is_some()
                || final_decision.recovery_handoff_id.is_some()
                || final_decision.return_intake_id.is_some()
                || final_decision.original_round.is_some()
                || final_decision.take_work_id.is_some()
                || final_decision.take_delivery_id.is_some()
            {
                return Err(ConsignmentError::Unavailable);
            }
            let return_mail_id =
                final_decision.return_mail_id.ok_or(ConsignmentError::Unavailable)?;
            let return_evidence_id =
                final_decision.return_evidence_id.ok_or(ConsignmentError::Unavailable)?;
            if return_mail_id.is_nil()
                || return_evidence_id.is_nil()
                || final_decision.obligation_id.is_some_and(|id| id.is_nil())
            {
                return Err(ConsignmentError::Unavailable);
            }
            QuarantineReleaseOutcome::AuctionReturnCase {
                return_mail_id,
                return_evidence_id,
                obligation_id: final_decision.obligation_id,
            }
        }
        ReleaseKind::AuctionRecoveryReturn => {
            if !final_decision.listing_changes.is_empty()
                || final_decision.return_evidence_id.is_some()
                || final_decision.return_intake_id.is_some()
                || final_decision.original_round.is_some()
                || final_decision.take_work_id.is_some()
                || final_decision.take_delivery_id.is_some()
            {
                return Err(ConsignmentError::Unavailable);
            }
            let obligation_id =
                final_decision.obligation_id.ok_or(ConsignmentError::Unavailable)?;
            let return_mail_id =
                final_decision.return_mail_id.ok_or(ConsignmentError::Unavailable)?;
            let recovery_id = final_decision.recovery_id.ok_or(ConsignmentError::Unavailable)?;
            let recovery_handoff_id =
                final_decision.recovery_handoff_id.ok_or(ConsignmentError::Unavailable)?;
            if obligation_id.is_nil()
                || return_mail_id.is_nil()
                || recovery_id.is_nil()
                || recovery_handoff_id.is_nil()
            {
                return Err(ConsignmentError::Unavailable);
            }
            QuarantineReleaseOutcome::AuctionRecoveryReturn {
                obligation_id,
                return_mail_id,
                recovery_id,
                recovery_handoff_id,
            }
        }
    };
    Ok(QuarantineReleaseRecord {
        release_id: final_decision.release_id,
        decision_id: final_decision.decision_id,
        released_at: final_decision.released_at,
        outcome,
    })
}

fn action_dto(action: AdminDepositAction) -> QuarantineAction {
    match action {
        AdminDepositAction::Recheck => QuarantineAction::Recheck,
        AdminDepositAction::ReleaseQuarantine => QuarantineAction::Release,
    }
}

fn reason_dto(reason: DomainQuarantineReason) -> QuarantineReason {
    match reason {
        DomainQuarantineReason::DuplicateRawEid => QuarantineReason::DuplicateRawEid,
        DomainQuarantineReason::CanonicalAliasAmbiguous => {
            QuarantineReason::CanonicalAliasAmbiguous
        }
        DomainQuarantineReason::UnknownAttachment => QuarantineReason::UnknownAttachment,
        DomainQuarantineReason::ListDetailConflict => QuarantineReason::ListDetailConflict,
        DomainQuarantineReason::AssetCountUnknown => QuarantineReason::AssetCountUnknown,
        DomainQuarantineReason::SameNameChangedUid => QuarantineReason::SameNameChangedUid,
        DomainQuarantineReason::IdentityUnproven => QuarantineReason::IdentityUnproven,
        DomainQuarantineReason::AmountUnproven => QuarantineReason::AmountUnproven,
        DomainQuarantineReason::ReturnUnproven => QuarantineReason::ReturnUnproven,
        DomainQuarantineReason::TakeUnproven => QuarantineReason::TakeUnproven,
        DomainQuarantineReason::UnsafeReturnShape => QuarantineReason::UnsafeReturnShape,
        DomainQuarantineReason::SuspectedSystemRts => QuarantineReason::SuspectedSystemRts,
    }
}

pub(super) fn recheck_dto(
    receipt: RecheckReceipt,
    quarantine_id: Uuid,
) -> Result<QuarantineRecheckReceipt, ConsignmentError> {
    if receipt.quarantine_id != quarantine_id || receipt.schema_version != 2 {
        return Err(ConsignmentError::OutcomeUnknown);
    }
    Ok(QuarantineRecheckReceipt {
        schema_version: 2,
        operation: ConsignmentOperationV2::RecheckQuarantine,
        disposition: ConsignmentDispositionV2::RecheckQueued,
        command_id: receipt.command_id.as_uuid(),
        quarantine_id: receipt.quarantine_id,
        work_id: receipt.work_id,
    })
}

pub(super) fn release_receipt_dto(
    receipt: ReleaseReceipt,
    quarantine_id: Uuid,
    command_id: Uuid,
    proposed: QuarantineDecisionKind,
) -> Result<QuarantineReleaseReceipt, ConsignmentError> {
    if receipt.quarantine_id != quarantine_id
        || receipt.schema_version != 2
        || receipt.command_id.as_uuid() != command_id
        || release_kind_dto(receipt.decision_kind) != proposed
    {
        return Err(ConsignmentError::OutcomeUnknown);
    }
    let common = (
        receipt.command_id.as_uuid(),
        receipt.quarantine_id,
        receipt.release_id,
        receipt.decision_id,
    );
    Ok(match receipt.decision_kind {
        ReleaseKind::Accept => QuarantineReleaseReceipt::Accept {
            schema_version: 2,
            operation: ConsignmentOperationV2::ReleaseQuarantine,
            disposition: ConsignmentDispositionV2::QuarantineReleased,
            command_id: common.0,
            quarantine_id: common.1,
            release_id: common.2,
            decision_id: common.3,
            listing_changes: receipt
                .listing_changes
                .into_iter()
                .map(stamp_dto)
                .collect::<Result<_, _>>()?,
        },
        ReleaseKind::Return => QuarantineReleaseReceipt::Return {
            schema_version: 2,
            operation: ConsignmentOperationV2::ReleaseQuarantine,
            disposition: ConsignmentDispositionV2::QuarantineReleased,
            command_id: common.0,
            quarantine_id: common.1,
            release_id: common.2,
            decision_id: common.3,
        },
        ReleaseKind::ArchiveEmpty => QuarantineReleaseReceipt::ArchiveEmpty {
            schema_version: 2,
            operation: ConsignmentOperationV2::ReleaseQuarantine,
            disposition: ConsignmentDispositionV2::QuarantineReleased,
            command_id: common.0,
            quarantine_id: common.1,
            release_id: common.2,
            decision_id: common.3,
        },
        ReleaseKind::AcquireReturn => QuarantineReleaseReceipt::AcquireReturn {
            schema_version: 2,
            operation: ConsignmentOperationV2::ReleaseQuarantine,
            disposition: ConsignmentDispositionV2::QuarantineReleased,
            command_id: common.0,
            quarantine_id: common.1,
            release_id: common.2,
            decision_id: common.3,
            return_intake_id: receipt.return_intake_id.ok_or(ConsignmentError::OutcomeUnknown)?,
            original_round: i64::from(
                receipt.original_round.ok_or(ConsignmentError::OutcomeUnknown)?,
            ),
            take_work_id: receipt.take_work_id.ok_or(ConsignmentError::OutcomeUnknown)?,
            take_delivery_id: receipt.take_delivery_id.ok_or(ConsignmentError::OutcomeUnknown)?,
        },
        ReleaseKind::AuctionReturnCase => QuarantineReleaseReceipt::AuctionReturnCase {
            schema_version: 2,
            operation: ConsignmentOperationV2::ReleaseQuarantine,
            disposition: ConsignmentDispositionV2::QuarantineReleased,
            command_id: common.0,
            quarantine_id: common.1,
            release_id: common.2,
            decision_id: common.3,
            return_mail_id: receipt.return_mail_id.ok_or(ConsignmentError::OutcomeUnknown)?,
            return_evidence_id: receipt
                .return_evidence_id
                .ok_or(ConsignmentError::OutcomeUnknown)?,
            obligation_id: receipt.obligation_id,
        },
        ReleaseKind::AuctionRecoveryReturn => QuarantineReleaseReceipt::AuctionRecoveryReturn {
            schema_version: 2,
            operation: ConsignmentOperationV2::ReleaseQuarantine,
            disposition: ConsignmentDispositionV2::QuarantineReleased,
            command_id: common.0,
            quarantine_id: common.1,
            release_id: common.2,
            decision_id: common.3,
            obligation_id: receipt.obligation_id.ok_or(ConsignmentError::OutcomeUnknown)?,
            return_mail_id: receipt.return_mail_id.ok_or(ConsignmentError::OutcomeUnknown)?,
            recovery_id: receipt.recovery_id.ok_or(ConsignmentError::OutcomeUnknown)?,
            recovery_handoff_id: receipt
                .recovery_handoff_id
                .ok_or(ConsignmentError::OutcomeUnknown)?,
        },
    })
}

fn stamp_dto(change: ListingStateChange) -> Result<ListingStampV2, ConsignmentError> {
    if change.state != ListingState::DepositAccepted {
        return Err(ConsignmentError::OutcomeUnknown);
    }
    Ok(ListingStampV2 {
        listing_id: change.listing_id,
        listing_version: input::listing_version(
            i64::try_from(change.listing_version).map_err(outcome_unknown)?,
        )?,
        state: ListingStateV2::DepositAccepted,
    })
}

pub(super) fn delist_receipt_dto(
    receipt: OperationsHoldReceipt,
) -> Result<ConsignmentDelistReceiptV2, ConsignmentError> {
    if receipt.schema_version != 2
        || receipt.operation != auction_application::consignment::DepositOperation::AdminDelist
    {
        return Err(ConsignmentError::OutcomeUnknown);
    }
    match receipt.disposition {
        ReceiptDisposition::Closed => {
            if receipt.hold_id.is_some() || receipt.state != ListingState::Closed {
                return Err(ConsignmentError::OutcomeUnknown);
            }
            Ok(ConsignmentDelistReceiptV2::Closed {
                schema_version: 2,
                operation: ConsignmentOperationV2::AdminDelist,
                command_id: receipt.command_id.as_uuid(),
                listing_id: receipt.listing_id,
                listing_version: input::listing_version(
                    i64::try_from(receipt.listing_version.value()).map_err(outcome_unknown)?,
                )?,
                state: ListingStateV2::Closed,
            })
        }
        ReceiptDisposition::HoldSet => {
            if receipt.state != ListingState::HeldOffMarket {
                return Err(ConsignmentError::OutcomeUnknown);
            }
            let hold_id = receipt.hold_id.ok_or(ConsignmentError::OutcomeUnknown)?;
            Ok(ConsignmentDelistReceiptV2::HoldSet {
                schema_version: 2,
                operation: ConsignmentOperationV2::AdminDelist,
                command_id: receipt.command_id.as_uuid(),
                listing_id: receipt.listing_id,
                listing_version: input::listing_version(
                    i64::try_from(receipt.listing_version.value()).map_err(outcome_unknown)?,
                )?,
                state: ListingStateV2::HeldOffMarket,
                hold_id,
            })
        }
        _ => Err(ConsignmentError::OutcomeUnknown),
    }
}

pub(super) fn hold_dto(
    receipt: OperationsHoldReceipt,
) -> Result<ConsignmentHoldReceipt, ConsignmentError> {
    if receipt.schema_version != 2
        || receipt.operation
            != auction_application::consignment::DepositOperation::ReleaseOperationsHold
        || receipt.disposition != ReceiptDisposition::HoldReleased
        || receipt.state != ListingState::HeldOffMarket
    {
        return Err(ConsignmentError::OutcomeUnknown);
    }
    let hold_id = receipt.hold_id.ok_or(ConsignmentError::OutcomeUnknown)?;
    Ok(ConsignmentHoldReceipt {
        schema_version: 2,
        operation: ConsignmentOperationV2::ReleaseHold,
        disposition: ConsignmentDispositionV2::HoldReleased,
        command_id: receipt.command_id.as_uuid(),
        listing_id: receipt.listing_id,
        listing_version: input::listing_version(
            i64::try_from(receipt.listing_version.value()).map_err(outcome_unknown)?,
        )?,
        state: ListingStateV2::HeldOffMarket,
        hold_id,
    })
}

pub(super) fn release_kind(kind: QuarantineDecisionKind) -> ReleaseKind {
    match kind {
        QuarantineDecisionKind::Accept => ReleaseKind::Accept,
        QuarantineDecisionKind::Return => ReleaseKind::Return,
        QuarantineDecisionKind::ArchiveEmpty => ReleaseKind::ArchiveEmpty,
        QuarantineDecisionKind::AcquireReturn => ReleaseKind::AcquireReturn,
        QuarantineDecisionKind::AuctionReturnCase => ReleaseKind::AuctionReturnCase,
        QuarantineDecisionKind::AuctionRecoveryReturn => ReleaseKind::AuctionRecoveryReturn,
    }
}

fn release_kind_dto(kind: ReleaseKind) -> QuarantineDecisionKind {
    match kind {
        ReleaseKind::Accept => QuarantineDecisionKind::Accept,
        ReleaseKind::Return => QuarantineDecisionKind::Return,
        ReleaseKind::ArchiveEmpty => QuarantineDecisionKind::ArchiveEmpty,
        ReleaseKind::AcquireReturn => QuarantineDecisionKind::AcquireReturn,
        ReleaseKind::AuctionReturnCase => QuarantineDecisionKind::AuctionReturnCase,
        ReleaseKind::AuctionRecoveryReturn => QuarantineDecisionKind::AuctionRecoveryReturn,
    }
}

pub(super) fn quarantine_version(value: i64) -> Result<u64, ConsignmentError> {
    u64::try_from(value).map_err(invalid_deposit)
}

fn unavailable<T>(_error: T) -> ConsignmentError {
    ConsignmentError::Unavailable
}

fn outcome_unknown<T>(_error: T) -> ConsignmentError {
    ConsignmentError::OutcomeUnknown
}

fn invalid_deposit<T>(_error: T) -> ConsignmentError {
    ConsignmentError::InvalidDepositRequest
}

fn wire_world(world: SettlementWorld) -> ListingWorld {
    match world {
        SettlementWorld::Main => ListingWorld::Main,
        SettlementWorld::Isekai => ListingWorld::Isekai,
    }
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
