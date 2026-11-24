//! 整封决定的接受、退回、隔离、待定与旧 owner 分流测试。

use auction_time::{Duration, Timestamp};
use uuid::Uuid;

use super::{
    EnvelopeCapabilities, EnvelopeDecision, EnvelopeItemKind, EnvelopeItemObservation,
    EnvelopeObservation, EnvelopeRoute, LegacyAuctionOwner, LockedEnvelopeFacts, PendingReason,
    QuarantineReason, ReservationBinding, ReservationState, ReturnItemReason, SenderIdentity,
    decide_envelope,
};
use crate::SettlementWorld;
use crate::UserId;
use crate::consignment::ListingEquipmentIdentity;
use crate::consignment::ListingVersion;
use crate::delivery::OriginalMailKey;

const NOW: Timestamp = Timestamp::UNIX_EPOCH;

fn user(value: u128) -> UserId {
    UserId::from_uuid(Uuid::from_u128(value))
}

fn equipment(world: &str, eid: &str) -> ListingEquipmentIdentity {
    ListingEquipmentIdentity::new(world, eid).expect("测试装备身份")
}

fn item(ordinal: u32, eid: &str) -> EnvelopeItemObservation {
    EnvelopeItemObservation {
        ordinal,
        kind: EnvelopeItemKind::Equipment,
        display_name: Some(format!("Fake Gear {ordinal}")),
        raw_eid: Some(eid.to_owned()),
        canonical_eid: Some(eid.to_owned()),
        amount: None,
        world: Some(SettlementWorld::Main),
        seller_user_id: Some(user(7)),
        tradeable: true,
        history_ok: true,
        protected_ref: Some(Uuid::from_u128(100 + u128::from(ordinal))),
    }
}

fn reservation(index: u32, eid: &str) -> ReservationBinding {
    ReservationBinding {
        reservation_id: Uuid::from_u128(1_000 + u128::from(index)),
        generation: 1,
        listing_id: Uuid::from_u128(2_000 + u128::from(index)),
        listing_version: ListingVersion::new(3).expect("测试版本"),
        seller_user_id: user(7),
        equipment: equipment("main", eid),
        expires_at: NOW + Duration::hours(1),
        state: ReservationState::Active,
    }
}

fn observation(items: Vec<EnvelopeItemObservation>) -> EnvelopeObservation {
    EnvelopeObservation {
        observation_id: Uuid::from_u128(9),
        session_binding_digest: [1; 32],
        manifest_digest: [2; 32],
        parser_version: "test".to_owned(),
        subject_is_consign: true,
        sender_identity: SenderIdentity::Verified,
        list_detail_consistent: true,
        asset_count: Some(u32::try_from(items.len()).expect("件数")),
        duplicate_raw_eid: false,
        canonical_alias_ambiguous: false,
        unknown_attachment: false,
        credits: Some(0),
        cod: Some(0),
        returnable: true,
        takeable: Some(true),
        system_rts: false,
        legacy_auction_owner: None,
        items,
    }
}

fn facts(
    observation: EnvelopeObservation,
    reservations: Vec<ReservationBinding>,
) -> LockedEnvelopeFacts {
    LockedEnvelopeFacts {
        mail_key: Some(
            OriginalMailKey::new("42", SettlementWorld::Main, "100").expect("测试原信键"),
        ),
        observation,
        reservations,
        envelope_seller_user_id: Some(user(7)),
        capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
        read_failure: false,
        already_decided: false,
        now: NOW,
    }
}

#[test]
fn accepts_only_one_to_one_whole_envelopes() {
    let route = decide_envelope(&facts(
        observation(vec![item(1, "42"), item(2, "43")]),
        vec![reservation(1, "42"), reservation(2, "43")],
    ));
    let EnvelopeRoute::Consignment(EnvelopeDecision::Accept(items)) = route else {
        panic!("应整封接受，实际 {route:?}");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].ordinal, 1);
    assert_eq!(items[1].ordinal, 2);
    assert_eq!(items[0].reservation_id, Uuid::from_u128(1_001));
}

#[test]
fn take_readiness_failures_quarantine_without_accepting() {
    let cases: [(&str, Option<bool>, bool); 3] = [
        ("world validator unsigned", Some(true), false),
        ("observed takeable false", Some(false), true),
        ("observed takeable unknown", None, true),
    ];
    for (name, takeable, take_ready) in cases {
        let mut observed = observation(vec![item(1, "42")]);
        observed.takeable = takeable;
        let mut locked = facts(observed, vec![reservation(1, "42")]);
        locked.capabilities.take_ready = take_ready;
        assert_eq!(
            decide_envelope(&locked),
            EnvelopeRoute::Consignment(EnvelopeDecision::Quarantine(
                QuarantineReason::TakeUnproven
            )),
            "{name}"
        );
    }
}

#[test]
fn asset_count_mismatch_in_either_direction_quarantines() {
    for asset_count in [1, 3] {
        let mut observed = observation(vec![item(1, "42"), item(2, "43")]);
        observed.asset_count = Some(asset_count);
        assert_eq!(
            decide_envelope(&facts(observed, vec![reservation(1, "42"), reservation(2, "43")])),
            EnvelopeRoute::Consignment(EnvelopeDecision::Quarantine(
                QuarantineReason::AssetCountUnknown
            )),
            "asset_count {asset_count}"
        );
    }
}

#[test]
fn one_non_tradeable_item_returns_the_whole_envelope() {
    let mut items = vec![item(1, "42"), item(2, "43")];
    items[1].tradeable = false;
    let route = decide_envelope(&facts(
        observation(items),
        vec![reservation(1, "42"), reservation(2, "43")],
    ));
    let EnvelopeRoute::Consignment(EnvelopeDecision::Return(manifest)) = route else {
        panic!("一件不可交易时不得接受，实际 {route:?}");
    };
    assert_eq!(manifest.items.len(), 2);
    assert_eq!(manifest.items[1].reason, ReturnItemReason::NotTradeable);
}

#[test]
fn last_item_without_reservation_returns_whole_envelope_without_accepting() {
    let route = decide_envelope(&facts(
        observation(vec![item(1, "42"), item(2, "43")]),
        vec![reservation(1, "42")],
    ));
    let EnvelopeRoute::Consignment(EnvelopeDecision::Return(manifest)) = route else {
        panic!("应整封退回，实际 {route:?}");
    };
    assert_eq!(manifest.items.len(), 2);
    assert_eq!(manifest.items[0].reason, ReturnItemReason::ReservationCountMismatch);
    assert_eq!(manifest.items[1].reason, ReturnItemReason::Unregistered);
}

#[test]
fn every_unreliable_shape_quarantines() {
    type Mutation = fn(&mut EnvelopeObservation);
    let cases: [(&str, Mutation, QuarantineReason); 8] = [
        ("duplicate raw eid", |o| o.duplicate_raw_eid = true, QuarantineReason::DuplicateRawEid),
        (
            "canonical alias",
            |o| o.canonical_alias_ambiguous = true,
            QuarantineReason::CanonicalAliasAmbiguous,
        ),
        (
            "unknown attachment",
            |o| o.unknown_attachment = true,
            QuarantineReason::UnknownAttachment,
        ),
        (
            "list detail conflict",
            |o| o.list_detail_consistent = false,
            QuarantineReason::ListDetailConflict,
        ),
        ("asset count", |o| o.asset_count = None, QuarantineReason::AssetCountUnknown),
        (
            "same name new uid",
            |o| o.sender_identity = SenderIdentity::SameNameDifferentUid,
            QuarantineReason::SameNameChangedUid,
        ),
        ("amount unknown", |o| o.credits = None, QuarantineReason::AmountUnproven),
        ("system rts", |o| o.system_rts = true, QuarantineReason::SuspectedSystemRts),
    ];
    for (name, mutate, expected) in cases {
        let mut observed = observation(vec![item(1, "42")]);
        mutate(&mut observed);
        let route = decide_envelope(&facts(observed, vec![reservation(1, "42")]));
        assert_eq!(
            route,
            EnvelopeRoute::Consignment(EnvelopeDecision::Quarantine(expected)),
            "{name}"
        );
    }
}

#[test]
fn unreadable_or_identityless_mail_stays_pending() {
    let mut unreadable = facts(observation(vec![item(1, "42")]), vec![reservation(1, "42")]);
    unreadable.read_failure = true;
    assert_eq!(
        decide_envelope(&unreadable),
        EnvelopeRoute::Pending(PendingReason::TransientReadFailure)
    );
    let mut no_uid = facts(observation(vec![item(1, "42")]), vec![reservation(1, "42")]);
    no_uid.mail_key = None;
    assert_eq!(
        decide_envelope(&no_uid),
        EnvelopeRoute::Pending(PendingReason::MissingAuthenticatedUid)
    );
    let mut decided = facts(observation(vec![item(1, "42")]), Vec::new());
    decided.already_decided = true;
    assert_eq!(decide_envelope(&decided), EnvelopeRoute::Pending(PendingReason::ExistingDecision));
}

#[test]
fn legacy_auction_owner_precedes_every_consignment_decision() {
    let mut observed = observation(vec![item(1, "42")]);
    observed.legacy_auction_owner = Some(LegacyAuctionOwner::BuyerRefund);
    observed.system_rts = true;
    assert_eq!(
        decide_envelope(&facts(observed, Vec::new())),
        EnvelopeRoute::LegacyAuction(LegacyAuctionOwner::BuyerRefund)
    );
}

#[test]
fn suspected_system_rts_is_quarantined_even_without_a_consign_subject() {
    let mut observed = observation(vec![item(1, "42")]);
    observed.subject_is_consign = false;
    observed.system_rts = true;
    assert_eq!(
        decide_envelope(&facts(observed, Vec::new())),
        EnvelopeRoute::Consignment(EnvelopeDecision::Quarantine(
            QuarantineReason::SuspectedSystemRts
        ))
    );
}

#[test]
fn credits_or_cod_are_never_accepted() {
    for (credits, cod) in [(Some(1), Some(0)), (Some(0), Some(1)), (Some(5), Some(6))] {
        let mut observed = observation(vec![item(1, "42")]);
        observed.credits = credits;
        observed.cod = cod;
        let route = decide_envelope(&facts(observed, vec![reservation(1, "42")]));
        assert!(matches!(route, EnvelopeRoute::Consignment(EnvelopeDecision::Return(_))));
    }
}

#[test]
fn unsafe_return_shape_is_quarantined_instead_of_returned() {
    let mut observed = observation(vec![item(1, "42")]);
    observed.credits = Some(1);
    let mut locked = facts(observed, vec![reservation(1, "42")]);
    locked.capabilities.safe_original_return = false;
    assert_eq!(
        decide_envelope(&locked),
        EnvelopeRoute::Consignment(EnvelopeDecision::Quarantine(QuarantineReason::ReturnUnproven))
    );
}

#[test]
fn deadline_equality_is_expired_and_prevents_acceptance() {
    let mut expired = reservation(1, "42");
    expired.expires_at = NOW;
    let route = decide_envelope(&facts(observation(vec![item(1, "42")]), vec![expired]));
    assert!(matches!(
        route,
        EnvelopeRoute::Consignment(EnvelopeDecision::Return(manifest))
            if manifest.items[0].reason == ReturnItemReason::ReservationExpired
    ));
}

#[test]
fn empty_verified_mail_archives_without_assets() {
    assert_eq!(
        decide_envelope(&facts(observation(Vec::new()), Vec::new())),
        EnvelopeRoute::Consignment(EnvelopeDecision::ArchiveEmpty)
    );
}
