//! 内存假实现的端口 trait 实现。

use std::future;

use auction_domain::SettlementWorld;
use auction_domain::consignment::deposit::{
    EnvelopeDecision, ReleaseKind, ReservationState, format_token,
};
use auction_domain::consignment::{ListingState, SeasonState, season_state};
use auction_domain::{CommandId, UserId};
use auction_time::Duration;
use uuid::Uuid;

use super::{
    FakeDepositStore, FakeObservation, FakeState, LedgerReceipt, ReservationRecord, SellerRecord,
    TerminalSlot, control::safe_items, reservation_receipt,
};
use crate::ports::consignment::deposit::{
    AppendQuarantineObservationCommand, DepositOperation, EnvelopeDisposition, EnvelopeReceipt,
    EnvelopeStore, ExpireReservationCommand, IssueReservationCommand, ObservationReceipt,
    QuarantineReference, RecheckQuarantineCommand, RecheckReceipt, ReleaseQuarantineCommand,
    ReleaseReceipt, ReservationReceipt, ReservationRetired, ReservationStore,
};
use crate::ports::consignment::{
    ConsignmentError, ConsignmentFuture, DepositBusinessRejection, ReservationInvalidation,
};
use crate::ports::core::RequestHash;

impl FakeDepositStore {
    pub(super) fn season_for(state: &FakeState) -> SeasonState {
        season_state(
            SettlementWorld::Isekai,
            state.frozen_season.as_ref(),
            state.current_season.as_ref(),
            state.clock,
        )
    }

    pub(super) fn active_reservation(
        state: &FakeState,
        listing_id: Uuid,
    ) -> Option<&ReservationRecord> {
        state.reservations.values().find(|reservation| {
            reservation.listing_id == listing_id
                && reservation.state == ReservationState::Active
                && state.clock < reservation.expires_at
        })
    }
}

impl ReservationStore for FakeDepositStore {
    fn issue_reservation(
        &self,
        command: IssueReservationCommand,
    ) -> ConsignmentFuture<'_, ReservationReceipt> {
        let result = self.perform(
            command.command_id,
            DepositOperation::IssueReservation,
            command.request_hash,
            Self::replay_reservation,
            |state| {
                let seller = state
                    .sellers
                    .get(&command.seller_user_id)
                    .copied()
                    .unwrap_or(SellerRecord { eligible: false, allowlisted: false });
                if state.bot_party {
                    return Err(DepositBusinessRejection::BotParty.into());
                }
                if !seller.eligible || !seller.allowlisted {
                    return Err(DepositBusinessRejection::ParticipantIneligible.into());
                }
                if !state.allowlisted {
                    return Err(DepositBusinessRejection::MailUnavailable.into());
                }
                if state.authenticated_world == SettlementWorld::Isekai {
                    match Self::season_for(state) {
                        SeasonState::Open | SeasonState::NotSeasonal => {}
                        _ => return Err(DepositBusinessRejection::SeasonClosed.into()),
                    }
                }
                let listing = state
                    .listings
                    .get(&command.listing_id)
                    .cloned()
                    .ok_or(DepositBusinessRejection::ListingNotActive)?;
                if listing.seller_user_id != command.seller_user_id {
                    return Err(DepositBusinessRejection::ListingNotActive.into());
                }
                if listing.version != command.expected_version.value() {
                    return Err(DepositBusinessRejection::StaleVersion.into());
                }
                if listing.state != ListingState::AwaitingDeposit {
                    return Err(DepositBusinessRejection::ListingNotActive.into());
                }
                let generation = state
                    .reservations
                    .values()
                    .filter(|reservation| reservation.listing_id == command.listing_id)
                    .map(|reservation| reservation.generation)
                    .max()
                    .unwrap_or(0)
                    + 1;
                let mut expired_claims = Vec::new();
                for reservation in state.reservations.values_mut() {
                    if reservation.listing_id == command.listing_id
                        && reservation.state == ReservationState::Active
                    {
                        reservation.state = ReservationState::Retired;
                        expired_claims.push(reservation.reservation_id);
                    }
                }
                state.claims.retain(|_, reservation_id| !expired_claims.contains(reservation_id));
                let reservation_id = Uuid::from_u128(state.next_id);
                state.next_id = state.next_id.saturating_add(1);
                let token = format_token([u8::try_from(generation).unwrap_or(255); 32]);
                let mut expires_at = state
                    .clock
                    .checked_add_signed(Duration::hours(24))
                    .ok_or(ConsignmentError::Unavailable)?;
                if state.authenticated_world == SettlementWorld::Isekai
                    && let Some(window) = state.frozen_season.as_ref()
                {
                    expires_at = expires_at.min(window.end_utc);
                }
                state.reservations.insert(
                    reservation_id,
                    ReservationRecord {
                        reservation_id,
                        generation,
                        listing_id: command.listing_id,
                        seller_user_id: command.seller_user_id,
                        canonical_eid: listing.canonical_eid.clone(),
                        token,
                        expires_at,
                        state: ReservationState::Active,
                    },
                );
                state.claims.insert(("main".to_owned(), listing.canonical_eid), reservation_id);
                let next_version = listing.version.saturating_add(1);
                if let Some(listing) = state.listings.get_mut(&command.listing_id) {
                    listing.version = next_version;
                }
                let receipt = reservation_receipt(
                    command.command_id,
                    command.listing_id,
                    next_version,
                    listing.state,
                    reservation_id,
                    expires_at,
                )?;
                Ok((LedgerReceipt::Reservation(receipt.clone()), receipt))
            },
        );
        Box::pin(future::ready(result))
    }

    fn expire_reservation(
        &self,
        command: ExpireReservationCommand,
    ) -> ConsignmentFuture<'_, ReservationRetired> {
        let result = self.perform(
            command.command_id,
            DepositOperation::ExpireReservation,
            command.request_hash,
            Self::replay_retired,
            |state| {
                let reservation = state
                    .reservations
                    .get(&command.reservation_id)
                    .cloned()
                    .ok_or(DepositBusinessRejection::ListingNotActive)?;
                if reservation.generation != command.generation {
                    return Err(DepositBusinessRejection::StaleVersion.into());
                }
                if reservation.state != ReservationState::Active {
                    return Err(DepositBusinessRejection::ListingNotActive.into());
                }
                if matches!(command.invalidation, ReservationInvalidation::Cancelled { .. }) {
                    // 裁定：取消只能作为挂单命令的随附事实，expire_reservation 不能单独声明。
                    return Err(DepositBusinessRejection::ActionNotAllowed.into());
                }
                if let ReservationInvalidation::Expired { expires_at } = command.invalidation
                    && state.clock < expires_at
                {
                    return Err(DepositBusinessRejection::ActionNotAllowed.into());
                }
                if let Some(record) = state.reservations.get_mut(&command.reservation_id) {
                    record.state = ReservationState::Retired;
                }
                state.claims.retain(|_, reservation_id| *reservation_id != command.reservation_id);
                if let Some(listing) = state.listings.get_mut(&reservation.listing_id) {
                    listing.version = listing.version.saturating_add(1);
                }
                let receipt = ReservationRetired::new(command.command_id, command.reservation_id);
                Ok((LedgerReceipt::Retired(receipt.clone()), receipt))
            },
        );
        Box::pin(future::ready(result))
    }

    fn replay_issue_reservation(
        &self,
        _seller_user_id: UserId,
        command_id: CommandId,
        _listing_id: Uuid,
        request_hash: RequestHash,
    ) -> ConsignmentFuture<'_, Option<ReservationReceipt>> {
        let result = self.replay(
            command_id,
            DepositOperation::IssueReservation,
            request_hash,
            Self::replay_reservation,
        );
        Box::pin(future::ready(result))
    }
}

impl EnvelopeStore for FakeDepositStore {
    fn decide_envelope(
        &self,
        command: crate::ports::consignment::DecideEnvelopeCommand,
    ) -> ConsignmentFuture<'_, EnvelopeReceipt> {
        let result = self.perform(
            command.command_id,
            DepositOperation::DecideEnvelope,
            command.request_hash,
            Self::replay_envelope,
            |state| {
                if let Some(existing) = state.terminals.get(&command.mail_key) {
                    if existing.decision == command.decision {
                        return Ok((
                            LedgerReceipt::Envelope(existing.receipt.clone()),
                            existing.receipt.clone(),
                        ));
                    }
                    return Err(DepositBusinessRejection::DispositionConflict.into());
                }
                let decision_id = Uuid::from_u128(state.next_id);
                state.next_id = state.next_id.saturating_add(1);
                let receipt = match &command.decision {
                    EnvelopeDecision::Quarantine(reason) => {
                        let quarantine_id = Uuid::from_u128(state.next_id);
                        state.next_id = state.next_id.saturating_add(1);
                        state.quarantines.insert(
                            quarantine_id,
                            super::FakeQuarantine {
                                quarantine_id,
                                mail_key: command.mail_key.clone(),
                                version: 1,
                                reason: *reason,
                                active: true,
                                observations: vec![FakeObservation {
                                    observation_id: command.facts.observation.observation_id,
                                    remote_observation_id: "first".to_owned(),
                                    observed_at: state.clock,
                                    parser_version: command
                                        .facts
                                        .observation
                                        .parser_version
                                        .clone(),
                                    digest: command.facts.observation.manifest_digest,
                                    credits: command.facts.observation.credits,
                                    cod: command.facts.observation.cod,
                                    returnable: command.facts.observation.returnable,
                                    takeable: command.facts.observation.takeable,
                                    items: safe_items(&command.facts.observation),
                                }],
                                owner: None,
                                release_receipt: None,
                            },
                        );
                        let mut receipt = EnvelopeReceipt::new(
                            command.command_id,
                            EnvelopeDisposition::Quarantined,
                            decision_id,
                        );
                        receipt.quarantine =
                            Some(QuarantineReference { quarantine_id, version: 1 });
                        receipt
                    }
                    EnvelopeDecision::Accept(items) => {
                        let mut changes = Vec::new();
                        for item in items {
                            let listing = state
                                .listings
                                .get(&item.listing_id)
                                .ok_or(DepositBusinessRejection::ListingNotActive)?;
                            if listing.version != item.listing_version.value() {
                                return Err(DepositBusinessRejection::StaleVersion.into());
                            }
                            if let Some(reservation) =
                                state.reservations.get_mut(&item.reservation_id)
                            {
                                reservation.state = ReservationState::Consumed;
                            }
                            state
                                .claims
                                .retain(|_, reservation_id| *reservation_id != item.reservation_id);
                            if let Some(listing) = state.listings.get_mut(&item.listing_id) {
                                listing.state = ListingState::DepositAccepted;
                                listing.version = listing.version.saturating_add(1);
                                changes.push(crate::ports::consignment::ListingStateChange {
                                    listing_id: item.listing_id,
                                    listing_version: listing.version,
                                    state: listing.state,
                                });
                            }
                        }
                        if changes.is_empty() {
                            return Err(DepositBusinessRejection::ActionNotAllowed.into());
                        }
                        let mut receipt = EnvelopeReceipt::new(
                            command.command_id,
                            EnvelopeDisposition::Accepted,
                            decision_id,
                        );
                        receipt.listing_changes = changes;
                        receipt
                    }
                    EnvelopeDecision::Return(_) => EnvelopeReceipt::new(
                        command.command_id,
                        EnvelopeDisposition::ReturnDecided,
                        decision_id,
                    ),
                    EnvelopeDecision::ArchiveEmpty => EnvelopeReceipt::new(
                        command.command_id,
                        EnvelopeDisposition::ArchivedEmpty,
                        decision_id,
                    ),
                };
                state.terminals.insert(
                    command.mail_key.clone(),
                    TerminalSlot { decision: command.decision.clone(), receipt: receipt.clone() },
                );
                Ok((LedgerReceipt::Envelope(receipt.clone()), receipt))
            },
        );
        Box::pin(future::ready(result))
    }

    fn append_quarantine_observation(
        &self,
        command: AppendQuarantineObservationCommand,
    ) -> ConsignmentFuture<'_, ObservationReceipt> {
        let result = self.perform(
            command.command_id,
            DepositOperation::AppendQuarantineObservation,
            command.request_hash,
            Self::replay_observation,
            |state| {
                let quarantine = state
                    .quarantines
                    .get_mut(&command.quarantine_id)
                    .ok_or(DepositBusinessRejection::ListingNotActive)?;
                if !quarantine.active {
                    return Err(DepositBusinessRejection::DispositionConflict.into());
                }
                if let Some(existing) = quarantine.observations.iter().find(|observation| {
                    observation.remote_observation_id == command.remote_observation_id
                }) {
                    if existing.digest != command.observation.manifest_digest {
                        return Err(DepositBusinessRejection::EvidenceConflict.into());
                    }
                    let receipt = ObservationReceipt::appended(
                        command.command_id,
                        command.quarantine_id,
                        quarantine.version,
                        existing.observation_id,
                        existing.digest,
                    );
                    return Ok((LedgerReceipt::Observation(receipt.clone()), receipt));
                }
                quarantine.version = quarantine.version.saturating_add(1);
                quarantine.observations.push(FakeObservation {
                    observation_id: command.observation.observation_id,
                    remote_observation_id: command.remote_observation_id.clone(),
                    observed_at: command.observed_at,
                    parser_version: command.observation.parser_version.clone(),
                    digest: command.observation.manifest_digest,
                    credits: command.observation.credits,
                    cod: command.observation.cod,
                    returnable: command.observation.returnable,
                    takeable: command.observation.takeable,
                    items: safe_items(&command.observation),
                });
                let receipt = ObservationReceipt::appended(
                    command.command_id,
                    command.quarantine_id,
                    quarantine.version,
                    command.observation.observation_id,
                    command.observation.manifest_digest,
                );
                Ok((LedgerReceipt::Observation(receipt.clone()), receipt))
            },
        );
        Box::pin(future::ready(result))
    }

    fn recheck_quarantine(
        &self,
        command: RecheckQuarantineCommand,
    ) -> ConsignmentFuture<'_, RecheckReceipt> {
        let result = self.perform(
            command.command_id,
            DepositOperation::RecheckQuarantine,
            command.request_hash,
            Self::replay_recheck,
            |state| {
                let quarantine = state
                    .quarantines
                    .get(&command.quarantine_id)
                    .ok_or(DepositBusinessRejection::ListingNotActive)?;
                if !quarantine.active {
                    return Err(DepositBusinessRejection::DispositionConflict.into());
                }
                if quarantine.version != command.expected_version {
                    return Err(DepositBusinessRejection::StaleVersion.into());
                }
                let work_id = Uuid::from_u128(state.next_id);
                state.next_id = state.next_id.saturating_add(1);
                let receipt =
                    RecheckReceipt::queued(command.command_id, command.quarantine_id, work_id);
                Ok((LedgerReceipt::Recheck(receipt.clone()), receipt))
            },
        );
        Box::pin(future::ready(result))
    }

    fn release_quarantine(
        &self,
        command: ReleaseQuarantineCommand,
    ) -> ConsignmentFuture<'_, ReleaseReceipt> {
        let result = self.perform(
            command.command_id,
            DepositOperation::ReleaseQuarantine,
            command.request_hash,
            Self::replay_release,
            |state| {
                if command.proposed_kind == ReleaseKind::AcquireReturn {
                    return Err(DepositBusinessRejection::EvidenceUnproven.into());
                }
                let quarantine = state
                    .quarantines
                    .get_mut(&command.quarantine_id)
                    .ok_or(DepositBusinessRejection::ListingNotActive)?;
                if !quarantine.active {
                    return Err(DepositBusinessRejection::DispositionConflict.into());
                }
                if quarantine.version != command.expected_version {
                    return Err(DepositBusinessRejection::StaleVersion.into());
                }
                if !quarantine
                    .observations
                    .iter()
                    .any(|observation| observation.observation_id == command.observation_id)
                {
                    return Err(DepositBusinessRejection::EvidenceUnproven.into());
                }
                if let Some(owner) = quarantine.owner {
                    if owner == command.proposed_kind
                        && let Some(receipt) = &quarantine.release_receipt
                    {
                        return Ok((LedgerReceipt::Release(receipt.clone()), receipt.clone()));
                    }
                    return Err(DepositBusinessRejection::DispositionConflict.into());
                }
                let release_id = Uuid::from_u128(state.next_id);
                state.next_id = state.next_id.saturating_add(1);
                let decision_id = Uuid::from_u128(state.next_id);
                state.next_id = state.next_id.saturating_add(1);
                let mut receipt = ReleaseReceipt::released(
                    command.command_id,
                    command.quarantine_id,
                    release_id,
                    decision_id,
                    command.proposed_kind,
                );
                match command.proposed_kind {
                    ReleaseKind::AuctionReturnCase => {
                        let return_mail_id = Uuid::from_u128(state.next_id);
                        state.next_id = state.next_id.saturating_add(1);
                        let evidence_id = Uuid::from_u128(state.next_id);
                        state.next_id = state.next_id.saturating_add(1);
                        receipt.return_mail_id = Some(return_mail_id);
                        receipt.return_evidence_id = Some(evidence_id);
                        state.settlement_owners = state.settlement_owners.saturating_add(1);
                    }
                    ReleaseKind::AuctionRecoveryReturn => {
                        let obligation_id = Uuid::from_u128(state.next_id);
                        state.next_id = state.next_id.saturating_add(1);
                        let return_mail_id = Uuid::from_u128(state.next_id);
                        state.next_id = state.next_id.saturating_add(1);
                        let recovery_id = Uuid::from_u128(state.next_id);
                        state.next_id = state.next_id.saturating_add(1);
                        let handoff_id = Uuid::from_u128(state.next_id);
                        state.next_id = state.next_id.saturating_add(1);
                        receipt.obligation_id = Some(obligation_id);
                        receipt.return_mail_id = Some(return_mail_id);
                        receipt.recovery_id = Some(recovery_id);
                        receipt.recovery_handoff_id = Some(handoff_id);
                        state.recovery_owners = state.recovery_owners.saturating_add(1);
                    }
                    ReleaseKind::Accept
                    | ReleaseKind::Return
                    | ReleaseKind::ArchiveEmpty
                    | ReleaseKind::AcquireReturn => {}
                }
                quarantine.active = false;
                quarantine.owner = Some(command.proposed_kind);
                quarantine.release_receipt = Some(receipt.clone());
                Ok((LedgerReceipt::Release(receipt.clone()), receipt))
            },
        );
        Box::pin(future::ready(result))
    }
}
