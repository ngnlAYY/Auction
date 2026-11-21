//! 自动清退证据、单页边界和启动代次同步。

use std::sync::Arc;

use auction_domain::LotId;
use auction_domain::consignment::{ListingMode, ListingReason, ListingSnapshot, ListingState};
use uuid::{Uuid, uuid};

use crate::ports::consignment::test_support::{FakeListingCommands, FakeLookupWork};
use crate::ports::consignment::{
    AutoCloseCandidate, AutoCloseEvidence, ConsignmentError, ListingAdmission,
    ListingAutoCloseEffect, ListingCommandOutcome, ListingCommandRejection, ListingMaintenancePage,
    StoredListingDecision, UnrecordedListingRejection,
};
use crate::services::consignment::encoding::auto_close_command;
use crate::services::consignment::tests::{LISTING_ID, price, run, time, version};
use crate::services::consignment::{ListingMaintenanceProgress, ListingMaintenanceService};

fn candidate(evidence: AutoCloseEvidence) -> AutoCloseCandidate {
    AutoCloseCandidate { listing_id: LISTING_ID, evidence }
}

#[test]
fn disabled_auto_close_identity_and_digest_depend_on_epoch() {
    let first = auto_close_command(candidate(AutoCloseEvidence::ServiceDisabled(2)));
    let next = auto_close_command(candidate(AutoCloseEvidence::ServiceDisabled(3)));
    assert_ne!(first.command_id, next.command_id);
    assert_ne!(first.request_hash, next.request_hash);
}

#[test]
fn sweep_advances_persistent_pages_and_preserves_auto_close_identity() {
    let work = Arc::new(FakeLookupWork::default());
    let commands = Arc::new(FakeListingCommands::default());
    let service = ListingMaintenanceService::new(work.clone(), commands.clone());
    let collision =
        candidate(AutoCloseEvidence::AuctionLotCreated(LotId::from_uuid(Uuid::from_u128(1))));
    let expired = candidate(AutoCloseEvidence::LookupDeadline(time(20)));
    let disabled = candidate(AutoCloseEvidence::ServiceDisabled(2));
    let seller_ineligible = candidate(AutoCloseEvidence::SellerIneligible(Uuid::from_u128(1)));
    let display_expired = candidate(AutoCloseEvidence::DisplayExpired(time(20)));
    let deposit_expired = candidate(AutoCloseEvidence::DepositExpired(time(20)));
    work.scan_candidates.reply(Ok(ListingMaintenancePage {
        scanned: 50,
        candidates: vec![collision, expired, disabled],
        scan_after: Some(LISTING_ID),
    }));
    work.scan_candidates.reply(Ok(ListingMaintenancePage {
        scanned: 4,
        candidates: vec![collision, seller_ineligible, display_expired, deposit_expired],
        scan_after: None,
    }));
    let terminated = ListingAutoCloseEffect::Terminated(ListingSnapshot {
        mode: ListingMode::Direct,
        state: ListingState::Closed,
        state_version: version(3),
        price: price(101),
        expires_at: time(100),
        reason: Some(ListingReason::AuctionLotCreated),
    });
    commands.auto_close.reply(Ok(ListingCommandOutcome::Accepted(terminated)));
    for _ in 0..2 {
        commands
            .auto_close
            .reply(Ok(ListingCommandOutcome::Accepted(ListingAutoCloseEffect::Unchanged)));
    }
    commands
        .auto_close
        .reply(Ok(ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(terminated))));
    for _ in 0..2 {
        commands
            .auto_close
            .reply(Ok(ListingCommandOutcome::Accepted(ListingAutoCloseEffect::ConflictsRecorded)));
    }
    commands.auto_close.reply(Ok(ListingCommandOutcome::Rejected(
        ListingCommandRejection::Unrecorded(UnrecordedListingRejection::ConditionNotMet),
    )));
    assert_eq!(
        run(service.sweep_page()),
        Ok(ListingMaintenanceProgress { scanned: 50, processed: 3, failed: 0, has_more: true })
    );
    assert_eq!(
        run(service.sweep_page()),
        Ok(ListingMaintenanceProgress { scanned: 4, processed: 3, failed: 0, has_more: false })
    );
    let calls = commands.auto_close.take_calls();
    assert_eq!(
        calls.iter().map(|call| call.candidate).collect::<Vec<_>>(),
        [
            collision,
            expired,
            disabled,
            collision,
            seller_ineligible,
            display_expired,
            deposit_expired
        ]
    );
    assert_eq!(calls[0], calls[3]);
    assert_eq!(calls[0].command_id.as_uuid(), uuid!("19235b81-fcfa-58ed-9c98-373c1da72551"));
    assert_eq!(
        calls[0].request_hash.as_bytes(),
        [
            179, 254, 37, 143, 91, 26, 234, 58, 135, 165, 38, 196, 164, 135, 54, 152, 216, 21, 212,
            240, 221, 197, 205, 42, 181, 118, 210, 167, 24, 203, 19, 206
        ],
    );
    for (index, first) in calls.iter().enumerate() {
        for (other, second) in calls.iter().enumerate().skip(index + 1) {
            if (index, other) != (0, 3) {
                assert_ne!(first.command_id, second.command_id);
                assert_ne!(first.request_hash, second.request_hash);
            }
        }
    }
    assert!(calls.iter().all(|call| !call.command_id.is_nil()));
    assert_eq!(work.scan_candidates.take_calls().len(), 2);
}

#[test]
fn sweep_empty_and_invalid_pages_do_not_issue_commands() {
    let work = Arc::new(FakeLookupWork::default());
    let commands = Arc::new(FakeListingCommands::default());
    let service = ListingMaintenanceService::new(work.clone(), commands.clone());
    work.scan_candidates.reply(Ok(ListingMaintenancePage {
        scanned: 0,
        candidates: vec![],
        scan_after: None,
    }));
    assert_eq!(
        run(service.sweep_page()),
        Ok(ListingMaintenanceProgress { scanned: 0, processed: 0, failed: 0, has_more: false })
    );
    work.scan_candidates.reply(Ok(ListingMaintenancePage {
        scanned: 50,
        candidates: vec![],
        scan_after: Some(LISTING_ID),
    }));
    assert_eq!(
        run(service.sweep_page()),
        Ok(ListingMaintenanceProgress { scanned: 50, processed: 0, failed: 0, has_more: true })
    );
    work.scan_candidates.reply(Ok(ListingMaintenancePage {
        scanned: 51,
        candidates: vec![],
        scan_after: None,
    }));
    assert_eq!(run(service.sweep_page()), Err(ConsignmentError::Unavailable));
    work.scan_candidates.reply(Ok(ListingMaintenancePage {
        scanned: 0,
        candidates: vec![candidate(AutoCloseEvidence::ServiceDisabled(2))],
        scan_after: None,
    }));
    assert_eq!(run(service.sweep_page()), Err(ConsignmentError::Unavailable));
    assert!(commands.auto_close.take_calls().is_empty());
}

#[test]
fn sweep_counts_command_failures_and_continues_with_later_listings() {
    let work = Arc::new(FakeLookupWork::default());
    let commands = Arc::new(FakeListingCommands::default());
    let service = ListingMaintenanceService::new(work.clone(), commands.clone());
    let first = candidate(AutoCloseEvidence::ServiceDisabled(2));
    let second = AutoCloseCandidate { listing_id: Uuid::from_u128(2), ..first };
    let third = AutoCloseCandidate { listing_id: Uuid::from_u128(3), ..first };
    let fourth = AutoCloseCandidate { listing_id: Uuid::from_u128(4), ..first };
    work.scan_candidates.reply(Ok(ListingMaintenancePage {
        scanned: 1,
        candidates: vec![first],
        scan_after: Some(LISTING_ID),
    }));
    commands
        .auto_close
        .reply(Ok(ListingCommandOutcome::Accepted(ListingAutoCloseEffect::Unchanged)));
    assert_eq!(
        run(service.sweep_page()),
        Ok(ListingMaintenanceProgress { scanned: 1, processed: 1, failed: 0, has_more: true })
    );
    work.scan_candidates.reply(Ok(ListingMaintenancePage {
        scanned: 4,
        candidates: vec![first, second, third, fourth],
        scan_after: None,
    }));
    commands.auto_close.reply(Ok(ListingCommandOutcome::Replayed(
        StoredListingDecision::Accepted(ListingAutoCloseEffect::Unchanged),
    )));
    commands.auto_close.reply(Err(ConsignmentError::OutcomeUnknown));
    commands.auto_close.reply(Ok(ListingCommandOutcome::Rejected(
        ListingCommandRejection::Unrecorded(UnrecordedListingRejection::CommandIdConflict),
    )));
    commands
        .auto_close
        .reply(Ok(ListingCommandOutcome::Accepted(ListingAutoCloseEffect::Unchanged)));
    assert_eq!(
        run(service.sweep_page()),
        Ok(ListingMaintenanceProgress { scanned: 4, processed: 2, failed: 2, has_more: false })
    );
    let calls = commands.auto_close.take_calls();
    assert_eq!(
        calls.iter().map(|call| call.candidate).collect::<Vec<_>>(),
        [first, first, second, third, fourth]
    );
    assert_eq!(calls[0], calls[1]);

    work.scan_candidates.reply(Err(ConsignmentError::Unavailable));
    assert_eq!(run(service.sweep_page()), Err(ConsignmentError::Unavailable));
    assert!(commands.auto_close.take_calls().is_empty());
    assert_eq!(work.scan_candidates.take_calls().len(), 3);
}

#[test]
fn sync_admission_requires_capability_and_a_committed_matching_epoch() {
    let work = Arc::new(FakeLookupWork::default());
    let service =
        ListingMaintenanceService::new(work.clone(), Arc::new(FakeListingCommands::default()));
    assert_eq!(run(service.sync_admission(true, false)), Err(ConsignmentError::Disabled));
    assert!(work.sync_admission.take_calls().is_empty());

    for (enabled, epoch) in [(false, 7), (false, 7), (true, 7)] {
        let admission = ListingAdmission { service_epoch: epoch, admitting: enabled };
        work.sync_admission.reply(Ok(admission));
        assert_eq!(run(service.sync_admission(enabled, enabled)), Ok(admission));
    }
    work.sync_admission.reply(Err(ConsignmentError::Unavailable));
    assert_eq!(run(service.sync_admission(false, false)), Err(ConsignmentError::Unavailable));
    for bad in [
        ListingAdmission { service_epoch: 7, admitting: true },
        ListingAdmission { service_epoch: 0, admitting: false },
    ] {
        work.sync_admission.reply(Ok(bad));
        assert_eq!(run(service.sync_admission(false, false)), Err(ConsignmentError::Unavailable));
    }
    assert_eq!(work.sync_admission.take_calls(), [false, false, true, false, false, false]);
    assert!(work.scan_candidates.take_calls().is_empty());
}
