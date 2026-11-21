//! 本人和运营命令的输入、原决定和安全拒绝。

use std::fmt::Debug;
use std::sync::Arc;

use auction_domain::CommandId;
use auction_domain::UserId;
use auction_domain::consignment::ListingState;
use uuid::Uuid;

use crate::ports::consignment::test_support::FakeListingCommands;
use crate::ports::consignment::{
    ConsignmentError, ListingBusinessRejection, ListingClosed, ListingCommandOutcome,
    ListingCommandRejection, ListingPriceDisposition, ListingPriceReceipt, StoredListingDecision,
    UnrecordedListingRejection,
};
use crate::services::consignment::ManageListingService;
use crate::services::consignment::tests::{
    FAKE_URL, LISTING_ID, SELLER_ID, admin, command_id, price, run, seller, version,
};

#[test]
fn reprice_and_withdraw_bind_the_same_authenticated_seller_but_distinct_operations() {
    let store = Arc::new(FakeListingCommands::default());
    let open = ManageListingService::new(store.clone(), true);
    let closed = ManageListingService::new(store.clone(), false);
    let seller = seller();
    let repriced = ListingPriceReceipt::new(
        LISTING_ID,
        version(3),
        ListingState::Resolving,
        price(101),
        ListingPriceDisposition::Repriced,
    )
    .unwrap();
    let unchanged = ListingPriceReceipt::new(
        LISTING_ID,
        version(3),
        ListingState::Resolving,
        price(101),
        ListingPriceDisposition::Unchanged,
    )
    .unwrap();
    store.reprice.reply(Ok(ListingCommandOutcome::Accepted(repriced)));
    store
        .reprice
        .reply(Ok(ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(repriced))));
    store.reprice.reply(Ok(ListingCommandOutcome::Accepted(unchanged)));
    assert_eq!(run(open.reprice(&seller, command_id(), LISTING_ID, 2, 101)), Ok(repriced));
    assert_eq!(run(closed.reprice(&seller, command_id(), LISTING_ID, 2, 101)), Ok(repriced));
    let new_command_id = CommandId::from_uuid(Uuid::from_u128(48));
    assert_eq!(run(open.reprice(&seller, new_command_id, LISTING_ID, 3, 101)), Ok(unchanged));
    let [first, replay, same_price] = store.reprice.take_calls().try_into().unwrap();
    assert_eq!(first.request_hash, replay.request_hash);
    assert_ne!(first.request_hash, same_price.request_hash);
    assert!(first.enabled);
    assert!(!replay.enabled);
    assert_eq!(first.price, price(101));
    assert_eq!(first.target.command_id, command_id());
    assert_eq!(first.target.listing_id, LISTING_ID);
    assert_eq!(first.target.seller_user_id, UserId::from_uuid(SELLER_ID));
    assert_eq!(first.target.expected_version, version(2));
    assert_eq!(same_price.target.command_id, new_command_id);
    assert_eq!(same_price.target.expected_version, version(3));
    assert_eq!(same_price.price, first.price);

    let receipt = ListingClosed::new(LISTING_ID, version(3), None).unwrap();
    store.withdraw.reply(Ok(ListingCommandOutcome::Accepted(receipt)));
    store
        .withdraw
        .reply(Ok(ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(receipt))));
    assert_eq!(run(open.withdraw(&seller, command_id(), LISTING_ID, 2)), Ok(receipt));
    assert_eq!(run(closed.withdraw(&seller, command_id(), LISTING_ID, 2)), Ok(receipt));
    let [withdraw, withdraw_replay] = store.withdraw.take_calls().try_into().unwrap();
    assert_eq!(withdraw.target, first.target);
    assert_eq!(withdraw, withdraw_replay);
    assert_ne!(withdraw.request_hash, first.request_hash);
}

#[test]
fn delist_binds_authenticated_actor_and_validated_reason() {
    let store = Arc::new(FakeListingCommands::default());
    let service = ManageListingService::new(store.clone(), false);
    let admin = admin();
    let receipt = ListingClosed::new(LISTING_ID, version(3), Some(price(101))).unwrap();
    for (outcome, reason) in [
        (ListingCommandOutcome::Accepted(receipt), "运营测试"),
        (ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(receipt)), "运营测试"),
        (
            ListingCommandOutcome::Rejected(ListingCommandRejection::Unrecorded(
                UnrecordedListingRejection::CommandIdConflict,
            )),
            "另一个说明",
        ),
    ] {
        let expected = if reason == "运营测试" {
            Ok(receipt)
        } else {
            Err(ConsignmentError::CommandIdConflict)
        };
        store.admin_delist.reply(Ok(outcome));
        assert_eq!(
            run(service.admin_delist(&admin, command_id(), LISTING_ID, 2, reason.to_owned())),
            expected
        );
    }
    let [first, replay, changed] = store.admin_delist.take_calls().try_into().unwrap();
    assert_eq!(first, replay);
    assert_eq!(first.admin_actor, admin.actor);
    assert_eq!(first.reason.as_str(), "运营测试");
    assert_eq!(first.command_id, command_id());
    assert_eq!(first.listing_id, LISTING_ID);
    assert_eq!(first.expected_version, version(2));
    assert_ne!(first.request_hash, changed.request_hash);
}

#[test]
fn manage_maps_each_authorization_version_and_command_rejection() {
    let store = Arc::new(FakeListingCommands::default());
    let service = ManageListingService::new(store.clone(), false);
    let seller = seller();
    let admin = admin();
    check_rejections(
        |outcome| store.reprice.reply(outcome),
        || run(service.reprice(&seller, command_id(), LISTING_ID, 2, 101)),
    );
    check_rejections(
        |outcome| store.withdraw.reply(outcome),
        || run(service.withdraw(&seller, command_id(), LISTING_ID, 2)),
    );
    check_rejections(
        |outcome| store.admin_delist.reply(outcome),
        || run(service.admin_delist(&admin, command_id(), LISTING_ID, 2, "运营测试".to_owned())),
    );
    for (outcome, expected) in [
        (
            Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Unrecorded(
                UnrecordedListingRejection::Disabled,
            ))),
            ConsignmentError::Disabled,
        ),
        (Err(ConsignmentError::Unavailable), ConsignmentError::Unavailable),
        (Err(ConsignmentError::OutcomeUnknown), ConsignmentError::OutcomeUnknown),
    ] {
        store.reprice.reply(outcome);
        assert_eq!(run(service.reprice(&seller, command_id(), LISTING_ID, 2, 101)), Err(expected));
    }
    assert_eq!(store.reprice.take_calls().len(), 10);
    assert_eq!(store.withdraw.take_calls().len(), 7);
    assert_eq!(store.admin_delist.take_calls().len(), 7);
}

fn check_rejections<T: Debug + PartialEq>(
    reply: impl Fn(Result<ListingCommandOutcome<T>, ConsignmentError>),
    execute: impl Fn() -> Result<T, ConsignmentError>,
) {
    use ListingBusinessRejection::{ListingNotActive, ListingNotFound, StaleVersion};
    for (error, expected) in [
        (ListingNotFound, (404, "listing_not_found")),
        (StaleVersion, (409, "stale_version")),
        (ListingNotActive, (409, "listing_not_active")),
    ] {
        for outcome in [
            ListingCommandOutcome::Rejected(ListingCommandRejection::Recorded(error)),
            ListingCommandOutcome::Replayed(StoredListingDecision::Rejected(error)),
        ] {
            reply(Ok(outcome));
            assert_eq!(execute().err().and_then(ConsignmentError::api_error), Some(expected));
        }
    }
    reply(Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Unrecorded(
        UnrecordedListingRejection::CommandIdConflict,
    ))));
    assert_eq!(
        execute().err().and_then(ConsignmentError::api_error),
        Some((409, "command_id_conflict"))
    );
}

#[test]
fn manage_rejects_invalid_inputs_before_writes() {
    let store = Arc::new(FakeListingCommands::default());
    let service = ManageListingService::new(store.clone(), true);
    let seller = seller();
    let admin = admin();
    assert_eq!(
        run(service.reprice(&seller, command_id(), LISTING_ID, 2, 0)),
        Err(ConsignmentError::InvalidPrice)
    );
    assert_eq!(
        run(service.withdraw(&seller, command_id(), LISTING_ID, 0)),
        Err(ConsignmentError::InvalidListingRequest)
    );
    assert_eq!(
        run(service.withdraw(&seller, command_id(), Uuid::nil(), 2)),
        Err(ConsignmentError::InvalidListingId)
    );
    assert_eq!(
        run(service.withdraw(&seller, CommandId::from_uuid(Uuid::nil()), LISTING_ID, 2)),
        Err(ConsignmentError::InvalidCommandId)
    );
    for input in ["", "多行\n说明", FAKE_URL] {
        assert_eq!(
            run(service.admin_delist(&admin, command_id(), LISTING_ID, 2, input.to_owned())),
            Err(ConsignmentError::InvalidReason)
        );
    }
    let mut invalid_admin = admin;
    invalid_admin.actor.clear();
    assert_eq!(
        run(service.admin_delist(
            &invalid_admin,
            command_id(),
            LISTING_ID,
            2,
            "运营测试".to_owned()
        )),
        Err(ConsignmentError::InvalidListingRequest)
    );
    let mut invalid_seller = seller;
    invalid_seller.user_id = Uuid::nil();
    assert_eq!(
        run(service.withdraw(&invalid_seller, command_id(), LISTING_ID, 2)),
        Err(ConsignmentError::InvalidListingRequest)
    );
    assert!(store.reprice.take_calls().is_empty());
    assert!(store.withdraw.take_calls().is_empty());
    assert!(store.admin_delist.take_calls().is_empty());
}

#[test]
fn reprice_hash_binds_target_version_and_price() {
    let store = Arc::new(FakeListingCommands::default());
    let service = ManageListingService::new(store.clone(), true);
    let seller = seller();
    let receipt = ListingPriceReceipt::new(
        LISTING_ID,
        version(3),
        ListingState::Resolving,
        price(101),
        ListingPriceDisposition::Repriced,
    )
    .unwrap();
    for (target, version, amount) in [
        (LISTING_ID, 2, 101),
        (Uuid::from_u128(98), 2, 101),
        (LISTING_ID, 3, 101),
        (LISTING_ID, 2, 102),
    ] {
        store.reprice.reply(Ok(ListingCommandOutcome::Accepted(receipt)));
        assert_eq!(
            run(service.reprice(&seller, command_id(), target, version, amount)),
            Ok(receipt)
        );
    }
    let [original, target_changed, version_changed, price_changed] =
        store.reprice.take_calls().try_into().unwrap();
    // v1 已持久化摘要的字节向量，变更编码必须另起版本。
    assert_eq!(
        original.request_hash.as_bytes(),
        [
            2, 115, 165, 85, 103, 176, 110, 249, 166, 134, 30, 251, 172, 42, 57, 166, 144, 19, 203,
            146, 73, 25, 193, 113, 202, 191, 56, 155, 145, 51, 154, 225
        ],
    );
    assert_ne!(original.request_hash, target_changed.request_hash);
    assert_ne!(original.request_hash, version_changed.request_hash);
    assert_ne!(original.request_hash, price_changed.request_hash);
}
