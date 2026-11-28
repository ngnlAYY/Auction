//! 内存假实现的外写、代管控制与季节任务。

use std::future;

use auction_domain::CommandId;
use auction_domain::consignment::deposit::EnvelopeObservation;
use auction_domain::consignment::{
    ListingState, decide_withdrawal_intent, operations_hold_reason, release_operations_hold_reason,
};
use uuid::Uuid;

use super::{FakeDepositStore, LedgerReceipt, WriteRecord};
use crate::ports::consignment::deposit::{
    AdminObservationItem, ApplyMailProofCommand, CustodyControlStore, DepositOperation,
    DispositionStore, OperationsHoldDisposition, OperationsHoldReceipt, ProofReceipt,
    ReleaseOperationsHoldCommand, SeasonCloseCommand, SeasonCloseReceipt, SeasonCloseTarget,
    SeasonNoticeCommand, SeasonNoticeReceipt, SeasonStore, SetOperationsHoldCommand,
    StartDispositionWriteCommand, StartReceipt, WithdrawDepositCommand, WithdrawDepositReceipt,
    WithdrawDisposition, WriteStartOutcome,
};
use crate::ports::consignment::{ConsignmentError, ConsignmentFuture, DepositBusinessRejection};
use crate::ports::core::RequestHash;

impl DispositionStore for FakeDepositStore {
    fn start_disposition_write(
        &self,
        command: StartDispositionWriteCommand,
    ) -> ConsignmentFuture<'_, WriteStartOutcome> {
        let result = (|| {
            let mut state = self.state.lock().expect("假实现状态锁");
            if let Some(record) = state.ledger.get(&command.command_id) {
                if record.operation != DepositOperation::StartDispositionWrite
                    || record.request_hash != command.request_hash
                {
                    return Err(ConsignmentError::CommandIdConflict);
                }
                if let LedgerReceipt::Start(receipt) = &record.receipt {
                    return Ok(WriteStartOutcome::AlreadyCommitted(receipt.clone()));
                }
                return Err(ConsignmentError::Unavailable);
            }
            if let Some(write) = state.writes.get(&command.work_id) {
                // 同一 work 必属同一最终决定与投递；不一致是确定冲突，不能回放别的工作的回执
                // （与 `start_port` 的工作行复核一致）。
                if write.final_decision_id != command.final_decision_id
                    || write.delivery_id != command.delivery_id
                {
                    return Err(DepositBusinessRejection::ActionNotAllowed.into());
                }
                let receipt = write.receipt.clone();
                state.ledger.insert(
                    command.command_id,
                    super::records::LedgerRecord {
                        operation: DepositOperation::StartDispositionWrite,
                        request_hash: command.request_hash,
                        receipt: LedgerReceipt::Start(receipt.clone()),
                    },
                );
                return Ok(WriteStartOutcome::AlreadyCommitted(receipt));
            }
            let snapshot = state.clone();
            let receipt = StartReceipt::started(
                command.command_id,
                command.final_decision_id,
                command.work_id,
                command.attempt_id,
                command.fence,
            );
            state.writes.insert(
                command.work_id,
                WriteRecord {
                    receipt: receipt.clone(),
                    final_decision_id: command.final_decision_id,
                    delivery_id: command.delivery_id,
                    fence: command.fence,
                },
            );
            state.permits_issued = state.permits_issued.saturating_add(1);
            if state.fail_next_commit {
                *state = snapshot;
                state.fail_next_commit = false;
                return Err(ConsignmentError::Unavailable);
            }
            state.ledger.insert(
                command.command_id,
                super::records::LedgerRecord {
                    operation: DepositOperation::StartDispositionWrite,
                    request_hash: command.request_hash,
                    receipt: LedgerReceipt::Start(receipt.clone()),
                },
            );
            if state.lose_next_response {
                state.lose_next_response = false;
                return Err(ConsignmentError::OutcomeUnknown);
            }
            // 假实现模拟存储适配器：只报告首次提交，许可由应用服务构造。
            Ok(WriteStartOutcome::FirstCommitted(receipt))
        })();
        Box::pin(future::ready(result))
    }

    fn apply_mail_proof(
        &self,
        command: ApplyMailProofCommand,
    ) -> ConsignmentFuture<'_, ProofReceipt> {
        let result = self.perform(
            command.command_id,
            DepositOperation::ApplyMailProof,
            command.request_hash,
            Self::replay_proof,
            |state| {
                let proof_id = Uuid::from_u128(state.next_id);
                state.next_id = state.next_id.saturating_add(1);
                let disposition = state.next_proof_disposition;
                let receipt = ProofReceipt::new(
                    command.command_id,
                    disposition,
                    command.final_decision_id,
                    proof_id,
                );
                Ok((LedgerReceipt::Proof(receipt.clone()), receipt))
            },
        );
        Box::pin(future::ready(result))
    }
}

impl CustodyControlStore for FakeDepositStore {
    fn request_withdrawal(
        &self,
        command: WithdrawDepositCommand,
    ) -> ConsignmentFuture<'_, WithdrawDepositReceipt> {
        let result = self.perform(
            command.command_id,
            DepositOperation::Withdraw,
            command.request_hash,
            Self::replay_withdraw,
            |state| {
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
                if let Some(intent_id) = listing.withdrawal_intent {
                    let receipt = WithdrawDepositReceipt::new(
                        command.command_id,
                        WithdrawDisposition::WithdrawalRequested,
                        command.listing_id,
                        command.expected_version,
                        listing.state,
                        Some(intent_id),
                    );
                    return Ok((LedgerReceipt::Withdraw(receipt.clone()), receipt));
                }
                match decide_withdrawal_intent(listing.state) {
                    Ok(outcome) => {
                        let intent_id = Uuid::from_u128(state.next_id);
                        state.next_id = state.next_id.saturating_add(1);
                        let next_state = match outcome {
                            auction_domain::consignment::deposit::WithdrawalIntentOutcome::MoveToWithdrawalPending => {
                                ListingState::WithdrawalPending
                            }
                            _ => listing.state,
                        };
                        let next_version = listing.version.saturating_add(1);
                        if let Some(record) = state.listings.get_mut(&command.listing_id) {
                            record.state = next_state;
                            record.version = next_version;
                            record.withdrawal_intent = Some(intent_id);
                        }
                        let receipt = WithdrawDepositReceipt::new(
                            command.command_id,
                            WithdrawDisposition::WithdrawalRequested,
                            command.listing_id,
                            auction_domain::consignment::ListingVersion::new(next_version)?,
                            next_state,
                            Some(intent_id),
                        );
                        Ok((LedgerReceipt::Withdraw(receipt.clone()), receipt))
                    }
                    Err(error) => {
                        if !matches!(
                            listing.state,
                            ListingState::Resolving
                                | ListingState::Displayed
                                | ListingState::AwaitingDeposit
                        ) {
                            return Err(error.into());
                        }
                        let next_version = listing.version.saturating_add(1);
                        if let Some(record) = state.listings.get_mut(&command.listing_id) {
                            record.state = ListingState::Closed;
                            record.version = next_version;
                            record.reason =
                                Some(auction_domain::consignment::ListingReason::SellerCancelled);
                            record.hold_id = None;
                            record.hold_reason = None;
                        }
                        let retired = state
                            .reservations
                            .values()
                            .filter(|reservation| {
                                reservation.listing_id == command.listing_id
                                    && reservation.state
                                        == auction_domain::consignment::deposit::ReservationState::Active
                            })
                            .map(|reservation| reservation.reservation_id)
                            .collect::<Vec<_>>();
                        for reservation in state.reservations.values_mut() {
                            if retired.contains(&reservation.reservation_id) {
                                reservation.state =
                                    auction_domain::consignment::deposit::ReservationState::Retired;
                            }
                        }
                        state.claims.retain(|_, reservation_id| !retired.contains(reservation_id));
                        let receipt = WithdrawDepositReceipt::new(
                            command.command_id,
                            WithdrawDisposition::Closed,
                            command.listing_id,
                            auction_domain::consignment::ListingVersion::new(next_version)?,
                            ListingState::Closed,
                            None,
                        );
                        Ok((LedgerReceipt::Withdraw(receipt.clone()), receipt))
                    }
                }
            },
        );
        Box::pin(future::ready(result))
    }

    fn set_operations_hold(
        &self,
        command: SetOperationsHoldCommand,
    ) -> ConsignmentFuture<'_, OperationsHoldReceipt> {
        let result = self.perform(
            command.command_id,
            DepositOperation::AdminDelist,
            command.request_hash,
            Self::replay_hold,
            |state| {
                let listing = state
                    .listings
                    .get(&command.listing_id)
                    .cloned()
                    .ok_or(DepositBusinessRejection::ListingNotActive)?;
                if listing.version != command.expected_version.value() {
                    return Err(DepositBusinessRejection::StaleVersion.into());
                }
                let next_version = listing.version.saturating_add(1);
                let receipt = match listing.state {
                    ListingState::Resolving
                    | ListingState::Displayed
                    | ListingState::AwaitingDeposit => {
                        let retired = state
                            .reservations
                            .values()
                            .filter(|reservation| {
                                reservation.listing_id == command.listing_id
                                    && reservation.state
                                        == auction_domain::consignment::deposit::ReservationState::Active
                            })
                            .map(|reservation| reservation.reservation_id)
                            .collect::<Vec<_>>();
                        for reservation in state.reservations.values_mut() {
                            if retired.contains(&reservation.reservation_id) {
                                reservation.state =
                                    auction_domain::consignment::deposit::ReservationState::Retired;
                            }
                        }
                        state.claims.retain(|_, reservation_id| !retired.contains(reservation_id));
                        if let Some(record) = state.listings.get_mut(&command.listing_id) {
                            record.state = ListingState::Closed;
                            record.version = next_version;
                            record.reason =
                                Some(auction_domain::consignment::ListingReason::AdminDelisted);
                            record.hold_id = None;
                            record.hold_reason = None;
                        }
                        OperationsHoldReceipt::new(
                            command.command_id,
                            OperationsHoldDisposition::Closed,
                            command.listing_id,
                            auction_domain::consignment::ListingVersion::new(next_version)?,
                            ListingState::Closed,
                            None,
                            None,
                        )?
                    }
                    ListingState::DepositAccepted
                    | ListingState::Available
                    | ListingState::HeldOffMarket => {
                        let reason = operations_hold_reason(listing.state)?;
                        let hold_id = Uuid::from_u128(state.next_id);
                        state.next_id = state.next_id.saturating_add(1);
                        if let Some(record) = state.listings.get_mut(&command.listing_id) {
                            record.state = ListingState::HeldOffMarket;
                            record.version = next_version;
                            record.hold_id = Some(hold_id);
                            record.hold_reason = Some(reason);
                        }
                        OperationsHoldReceipt::new(
                            command.command_id,
                            OperationsHoldDisposition::HoldSet,
                            command.listing_id,
                            auction_domain::consignment::ListingVersion::new(next_version)?,
                            ListingState::HeldOffMarket,
                            Some(hold_id),
                            Some(reason),
                        )?
                    }
                    _ => return Err(DepositBusinessRejection::ListingNotActive.into()),
                };
                Ok((LedgerReceipt::Hold(receipt.clone()), receipt))
            },
        );
        Box::pin(future::ready(result))
    }

    fn release_operations_hold(
        &self,
        command: ReleaseOperationsHoldCommand,
    ) -> ConsignmentFuture<'_, OperationsHoldReceipt> {
        let result = self.perform(
            command.command_id,
            DepositOperation::ReleaseOperationsHold,
            command.request_hash,
            Self::replay_hold,
            |state| {
                let listing = state
                    .listings
                    .get(&command.listing_id)
                    .cloned()
                    .ok_or(DepositBusinessRejection::ListingNotActive)?;
                if listing.version != command.expected_version.value() {
                    return Err(DepositBusinessRejection::StaleVersion.into());
                }
                let reason = release_operations_hold_reason(
                    listing.state,
                    listing.hold_id,
                    command.hold_id,
                )?;
                let next_version = listing.version.saturating_add(1);
                if let Some(record) = state.listings.get_mut(&command.listing_id) {
                    record.version = next_version;
                    record.hold_reason = Some(reason);
                }
                let receipt = OperationsHoldReceipt::new(
                    command.command_id,
                    OperationsHoldDisposition::HoldReleased,
                    command.listing_id,
                    auction_domain::consignment::ListingVersion::new(next_version)?,
                    ListingState::HeldOffMarket,
                    Some(command.hold_id),
                    Some(reason),
                )?;
                Ok((LedgerReceipt::Hold(receipt.clone()), receipt))
            },
        );
        Box::pin(future::ready(result))
    }

    fn replay_set_operations_hold(
        &self,
        _admin_actor: String,
        command_id: CommandId,
        _listing_id: Uuid,
        request_hash: RequestHash,
    ) -> ConsignmentFuture<'_, Option<OperationsHoldReceipt>> {
        Box::pin(future::ready(self.replay(
            command_id,
            DepositOperation::AdminDelist,
            request_hash,
            Self::replay_hold,
        )))
    }
}

impl SeasonStore for FakeDepositStore {
    fn queue_season_notice(
        &self,
        command: SeasonNoticeCommand,
    ) -> ConsignmentFuture<'_, SeasonNoticeReceipt> {
        let result = self.perform(
            command.command_id,
            DepositOperation::SeasonNotice,
            command.request_hash,
            Self::replay_notice,
            |state| {
                let key = (
                    command.position_id,
                    command.seller_user_id.as_uuid(),
                    command.notice_kind.as_str().to_owned(),
                );
                let work_id = if let Some(existing) = state.notices.get(&key) {
                    *existing
                } else {
                    let work_id = Uuid::from_u128(state.next_id);
                    state.next_id = state.next_id.saturating_add(1);
                    state.notices.insert(key, work_id);
                    work_id
                };
                let receipt = SeasonNoticeReceipt::queued(
                    command.command_id,
                    command.season.season_id.clone(),
                    command.position_id,
                    command.seller_user_id,
                    work_id,
                );
                Ok((LedgerReceipt::Notice(receipt.clone()), receipt))
            },
        );
        Box::pin(future::ready(result))
    }

    fn close_season(
        &self,
        command: SeasonCloseCommand,
    ) -> ConsignmentFuture<'_, SeasonCloseReceipt> {
        let result = self.perform(
            command.command_id,
            DepositOperation::SeasonClose,
            command.request_hash,
            Self::replay_close,
            |state| {
                // 冻结界不因任务迟到而延长；等号起归季后（D9 决定 26）。
                if state.clock < command.season.end_utc {
                    return Err(DepositBusinessRejection::ActionNotAllowed.into());
                }
                // 假实现不建模代管 position：无法把 position 解析成所属挂单，按生产
                // “position 不属于本季窗”失败关闭，不能悄悄关成零件。
                if matches!(command.target, SeasonCloseTarget::Position(_)) {
                    return Err(DepositBusinessRejection::SourceMismatch.into());
                }
                let mut closed = Vec::new();
                for (listing_id, listing) in state.listings.iter_mut() {
                    let close = match command.target {
                        SeasonCloseTarget::World(_) => true,
                        SeasonCloseTarget::Listing(target) => *listing_id == target,
                        // 已在上方失败关闭。
                        SeasonCloseTarget::Position(_) => false,
                    };
                    if close && listing.state.is_active() {
                        listing.state = ListingState::Closed;
                        listing.reason =
                            Some(auction_domain::consignment::ListingReason::SeasonClosed);
                        listing.version = listing.version.saturating_add(1);
                        closed.push(*listing_id);
                    }
                }
                let receipt = SeasonCloseReceipt::closed(
                    command.command_id,
                    command.season.season_id.clone(),
                    command.target,
                    closed,
                );
                Ok((LedgerReceipt::Close(receipt.clone()), receipt))
            },
        );
        Box::pin(future::ready(result))
    }
}

pub(super) fn safe_items(observation: &EnvelopeObservation) -> Vec<AdminObservationItem> {
    observation
        .items
        .iter()
        .map(|item| AdminObservationItem {
            ordinal: item.ordinal,
            kind: item.kind,
            canonical_eid: item.canonical_eid.clone(),
            amount: item.amount.and_then(|amount| i64::try_from(amount).ok()),
        })
        .collect()
}
