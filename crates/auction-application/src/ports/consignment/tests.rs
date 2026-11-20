//! 寄售端口值对象、互斥结果、安全映射及进程内测试替身回归。

use std::future::Future;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use auction_domain::consignment::{
    FeeBps, ListingEquipmentIdentity, ListingError, ListingMode, ListingPrice, ListingReason,
    ListingState, ListingVersion,
};
use auction_domain::{CommandId, EquipmentId, EquipmentKey, EquipmentKeyEnvelope, UserId};
use auction_time::Timestamp;
use uuid::{Uuid, uuid};

use crate::errors::ApplicationError;
use crate::ports::consignment::commands::{
    ListingClosed, ListingCommandOutcome, ListingCommandRejection, ListingCommandStore,
    ListingDelistReason, ListingOperation, ListingRegistered, ListingRegistrationPolicy,
    OwnedListingDecision, RegisterListingCommand, RegistrationInput, StoredListingDecision,
};
use crate::ports::consignment::equipment::{
    ListingEquipmentDetails, ListingEquipmentLabel, ListingEquipmentLevel, ListingEquipmentLookup,
    ListingEquipmentLookupError, ListingEquipmentReference, ListingEquipmentStat,
    ListingEquipmentStatGroup, ListingEquipmentUrlValidator, ListingLookupSnapshot,
    ListingRegistrationSecret, LookupPermanentReason, LookupRetryAfter, RegistrationRequestMac,
    WrappedRegistrationReplayKey,
};
use crate::ports::consignment::errors::{
    ConsignmentError, ListingBusinessRejection, UnrecordedListingRejection,
};
use crate::ports::consignment::maintenance::{ListingAdmission, ListingMaintenance};
use crate::ports::consignment::reads::{
    ListingCategoryFilter, ListingNameQuery, ListingPageWindow, ListingWorld, ListingWorlds,
    OwnedListingRead, PrivateListing, PrivateListingView,
};
use crate::ports::consignment::test_support::{
    FakeListingCommands, FakeListingEquipment, FakeListingReads, FakeLookupWork,
};
use crate::ports::consignment::work::{ListingLookupBudget, LookupClaimRequest, LookupFence};
use crate::ports::core::RepositoryError;

const WORK_ID: Uuid = Uuid::from_u128(12);
const LISTING_ID: Uuid = Uuid::from_u128(13);
const FAKE_TOKEN: Uuid = uuid!("12345678-1234-4234-8234-123456789abc");
const FAKE_EQUIPMENT_KEY: &str = "FA1234FA00";

// 固定夹具构造失败立即终止测试，避免将原始输入或错误链写入输出。
trait Fixture<T> {
    fn fixture(self) -> T;
}

impl<T> Fixture<T> for Option<T> {
    #[track_caller]
    fn fixture(self) -> T {
        let Some(value) = self else {
            panic!("固定测试值未产生");
        };
        value
    }
}

impl<T, E> Fixture<T> for Result<T, E> {
    #[track_caller]
    fn fixture(self) -> T {
        let Ok(value) = self else {
            panic!("固定测试值未成功产生");
        };
        value
    }
}

fn now() -> Timestamp {
    Timestamp::from_timestamp(1_800_000_000, 0).fixture()
}

fn identity() -> ListingEquipmentIdentity {
    ListingEquipmentIdentity::new("main", "42").fixture()
}

fn equipment_label() -> ListingEquipmentLabel {
    ListingEquipmentLabel {
        name: "测试装备".to_owned(),
        alt_name: None,
        category: "测试类别".to_owned(),
        level: ListingEquipmentLevel::Assigned(100_000),
        tradeable: true,
    }
}

fn stat(name: &str, value: f64, base: Option<f64>) -> ListingEquipmentStat {
    ListingEquipmentStat { name: name.to_owned(), value, base }
}

#[test]
fn domain_errors_map_to_safe_http_results() {
    let cases = [
        (ListingVersion::new(0).err().fixture(), 400, "invalid_listing_request"),
        (
            ListingVersion::new(ListingVersion::MAX + 1).err().fixture(),
            400,
            "invalid_listing_request",
        ),
        (ListingError::InvalidIdentity, 400, "invalid_equipment_url"),
        (ListingError::InvalidPrice, 400, "invalid_price"),
        (ListingError::InvalidFee, 400, "invalid_listing_request"),
        (
            ListingError::InvalidTransition { from: "closed", action: "reprice" },
            409,
            "listing_not_active",
        ),
        (
            ListingError::InvalidTransition { from: "rejected", action: "withdraw" },
            409,
            "listing_not_active",
        ),
        (
            ListingError::InvalidTransition { from: "expired", action: "admin_delist" },
            409,
            "listing_not_active",
        ),
        (
            ListingError::InvalidTransition { from: "closed", action: "decode_snapshot" },
            503,
            "consignment_unavailable",
        ),
        (
            ListingError::InvalidTransition { from: "displayed", action: "advance_version" },
            503,
            "consignment_unavailable",
        ),
        (
            ListingError::InvalidTransition {
                from: "resolving",
                action: "configure_publication_ttl",
            },
            503,
            "consignment_unavailable",
        ),
        (
            ListingError::InvalidTransition { from: "absent", action: "set_deadline" },
            503,
            "consignment_unavailable",
        ),
        (ListingError::StaleVersion { expected: 7, actual: 8 }, 409, "stale_version"),
        (ListingError::Ineligible, 403, "participant_ineligible"),
        (ListingError::EquipmentBusy, 409, "equipment_busy"),
        (ListingError::HistoryUnproven, 409, "history_unproven"),
        (ListingError::HistoryOwnerMismatch, 409, "history_owner_mismatch"),
        (ListingError::HistoryAliasConflict, 409, "history_alias_conflict"),
        (ListingError::AlreadyOpen, 409, "already_open"),
    ];
    for (domain, status, code) in cases {
        assert_eq!(ConsignmentError::from(domain).api_error(), Some((status, code)));
    }
}

#[test]
fn store_decisions_preserve_original_success_and_rejection() {
    use ListingBusinessRejection as Reject;

    let accepted =
        ListingRegistered { listing_id: LISTING_ID, price: ListingPrice::new(101).fixture() };
    let result = ListingCommandOutcome::Accepted(accepted).into_result();
    assert_eq!(result, Ok(accepted));
    assert_eq!(
        ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(accepted)).into_result(),
        Ok(accepted)
    );
    for (rejection, status, code) in [
        (Reject::ListingNotFound, 404, "listing_not_found"),
        (Reject::ParticipantIneligible, 403, "participant_ineligible"),
        (Reject::AlreadyOpen, 409, "already_open"),
        (Reject::EquipmentBusy, 409, "equipment_busy"),
        (Reject::HistoryUnproven, 409, "history_unproven"),
        (Reject::HistoryOwnerMismatch, 409, "history_owner_mismatch"),
        (Reject::HistoryAliasConflict, 409, "history_alias_conflict"),
        (Reject::FeeChanged, 409, "fee_changed"),
        (Reject::StaleVersion, 409, "stale_version"),
        (Reject::ListingNotActive, 409, "listing_not_active"),
        (Reject::WorldUnavailable, 409, "world_unavailable"),
        (Reject::RegistrationRateLimited, 429, "registration_rate_limited"),
        (Reject::ListingLimitReached, 429, "listing_limit_reached"),
    ] {
        let initial =
            ListingCommandOutcome::<()>::Rejected(ListingCommandRejection::Recorded(rejection));
        let replayed =
            ListingCommandOutcome::<()>::Replayed(StoredListingDecision::Rejected(rejection));
        assert_ne!(initial, replayed);
        assert_eq!(initial.into_result(), Err(ConsignmentError::Business(rejection)));
        assert_eq!(replayed.into_result().err().fixture().api_error(), Some((status, code)));
    }
}

#[test]
fn owned_receipts_keep_the_original_status_and_operation() {
    let rejected = OwnedListingDecision::Reprice(StoredListingDecision::Rejected(
        ListingBusinessRejection::StaleVersion,
    ));
    let accepted =
        OwnedListingDecision::Register(StoredListingDecision::Accepted(ListingRegistered {
            listing_id: LISTING_ID,
            price: ListingPrice::new(101).fixture(),
        }));
    let closed = OwnedListingDecision::Withdraw(StoredListingDecision::Accepted(ListingClosed {
        listing_id: LISTING_ID,
        version: ListingVersion::new(2).fixture(),
        price: None,
    }));
    for (decision, operation, status, code) in [
        (accepted, "register", 202, "accepted"),
        (rejected, "reprice", 409, "stale_version"),
        (closed, "withdraw", 200, "ok"),
    ] {
        assert_eq!(decision.operation().as_str(), operation);
        assert_eq!(decision.status_and_code(), (status, code));
    }
    assert_eq!(ListingOperation::ApplyLookup.as_str(), "apply_lookup");
    assert_eq!(ListingOperation::AutoClose.as_str(), "auto_close");
    assert_eq!(ListingOperation::AdminDelist.as_str(), "admin_delist");
}

#[test]
fn unrecorded_and_unknown_results_keep_distinct_errors() {
    use UnrecordedListingRejection as Reject;

    for (rejection, error, http) in [
        (
            Reject::CommandIdConflict,
            ConsignmentError::CommandIdConflict,
            Some((409, "command_id_conflict")),
        ),
        (Reject::Disabled, ConsignmentError::Disabled, Some((404, "consignment_disabled"))),
        (Reject::LeaseLost, ConsignmentError::LeaseLost, None),
        (Reject::ConditionNotMet, ConsignmentError::ConditionNotMet, None),
    ] {
        let outcome =
            ListingCommandOutcome::<()>::Rejected(ListingCommandRejection::Unrecorded(rejection));
        assert_eq!(outcome.into_result(), Err(error));
        assert_eq!(error.api_error(), http);
    }
    for (error, status, code) in [
        (ConsignmentError::InvalidCommandId, 400, "invalid_command_id"),
        (ConsignmentError::InvalidListingId, 400, "invalid_listing_id"),
        (ConsignmentError::InvalidQuery, 400, "invalid_query"),
        (ConsignmentError::InvalidReason, 400, "invalid_reason"),
        (ConsignmentError::CommandNotFound, 404, "command_not_found"),
        (ConsignmentError::Unavailable, 503, "consignment_unavailable"),
        (ConsignmentError::OutcomeUnknown, 503, "outcome_unknown"),
    ] {
        assert_eq!(error.api_error(), Some((status, code)));
    }
    let dependency_error = ConsignmentError::read_dependency_failure(ApplicationError::Repository(
        RepositoryError::Failed(FAKE_EQUIPMENT_KEY.to_owned()),
    ));
    assert_eq!(dependency_error, ConsignmentError::Unavailable);
    assert!(!format!("{dependency_error:?} {dependency_error}").contains(FAKE_EQUIPMENT_KEY));
}

#[test]
fn windows_honor_defaults_custom_sizes_and_offset_bounds() {
    let first = ListingPageWindow::new(None, None).fixture();
    assert_eq!((first.page(), first.page_size(), first.offset()), (1, 20, 0));
    let custom = ListingPageWindow::new(Some(3), Some(37)).fixture();
    assert_eq!((custom.page(), custom.page_size(), custom.offset()), (3, 37, 74));
    assert_eq!(ListingPageWindow::new(Some(51), Some(200)).fixture().offset(), 10_000);
    assert_eq!(ListingPageWindow::new(Some(10_001), Some(1)).fixture().offset(), 10_000);
    for (page, size) in [(0, 1), (1, 0), (1, 201), (52, 200), (u32::MAX, u32::MAX)] {
        assert_eq!(
            ListingPageWindow::new(Some(page), Some(size)),
            Err(ConsignmentError::InvalidQuery)
        );
    }
}

#[test]
fn filters_use_distinct_byte_and_character_budgets() {
    let sensitive_query = format!("https://example.invalid/equip/42/{FAKE_EQUIPMENT_KEY}");
    assert_eq!(
        format!("{:?}", ListingNameQuery::new(sensitive_query.clone()).fixture()),
        "[REDACTED]"
    );
    assert_eq!(
        format!("{:?}", ListingCategoryFilter::new(sensitive_query).fixture()),
        "[REDACTED]"
    );
    assert_eq!("main".parse::<ListingWorld>(), Ok(ListingWorld::Main));
    assert_eq!("isekai".parse::<ListingWorld>(), Ok(ListingWorld::Isekai));
    assert_eq!("Main".parse::<ListingWorld>(), Err(ConsignmentError::InvalidQuery));
    assert_eq!(ListingCategoryFilter::new("é".repeat(64)).fixture().as_str().len(), 128);
    assert_eq!(ListingCategoryFilter::new("é".repeat(65)), Err(ConsignmentError::InvalidQuery));
    assert_eq!(ListingNameQuery::new("装".repeat(80)).fixture().as_str().chars().count(), 80);
    assert_eq!(ListingNameQuery::new("装".repeat(81)), Err(ConsignmentError::InvalidQuery));
    assert_eq!(ListingNameQuery::new("A%_".to_owned()).fixture().as_str(), "A%_");
    for text in ["", " a", "a ", "a\nb", "a\u{2028}b"] {
        assert_eq!(ListingNameQuery::new(text.to_owned()), Err(ConsignmentError::InvalidQuery));
        assert_eq!(
            ListingCategoryFilter::new(text.to_owned()),
            Err(ConsignmentError::InvalidQuery)
        );
    }
}

#[test]
fn delist_reason_enforces_shape_and_keeps_text_private() {
    let text = "运营下架";
    let reason = ListingDelistReason::new(text.to_owned()).fixture();
    assert_eq!(reason.as_str(), text);
    assert_eq!(format!("{reason:?}"), "[REDACTED]");
    assert!(ListingDelistReason::new("é".repeat(128)).is_ok());
    for rejected in [
        String::new(),
        " 前导空格".to_owned(),
        "甲\u{2028}乙".to_owned(),
        "é".repeat(129),
        format!("说明 https://example.invalid/EQUIP/42/{FAKE_EQUIPMENT_KEY}"),
        "说明 HTTPS://example.invalid/EQUIP/42/fake-opaque".to_owned(),
        "说明 key=fake-opaque".to_owned(),
        format!("凭证 {FAKE_EQUIPMENT_KEY}"),
        format!("另一种凭证 {}", "x-".repeat(21) + "Y"),
    ] {
        assert_eq!(ListingDelistReason::new(rejected), Err(ConsignmentError::InvalidReason));
    }
}

#[test]
fn equipment_details_sort_and_validate_whitelisted_fields() {
    let make = |name: &str, stats| ListingEquipmentStatGroup { name: name.to_owned(), stats };
    let stats = vec![
        make("Z", vec![stat("乙", -1e9, None), stat("A", 1e9, Some(0.25))]),
        make("A", vec![stat("B", 1.5, None)]),
    ];
    let details = ListingEquipmentDetails::new(now(), equipment_label(), stats).fixture();
    assert_eq!(details.observed_at(), now());
    assert_eq!(details.stat_groups()[0].name, "A");
    assert_eq!(details.stat_groups()[1].stats[0].name, "A");
    assert_eq!(details.stat_groups()[1].stats[0].base, Some(0.25));
    assert_eq!(details.stat_groups()[1].stats[1].value, -1e9);
    for bad in [
        vec![make("Z", vec![]), make("Z", vec![])],
        vec![make("Z", vec![stat("A", 1.0, None), stat("A", 2.0, None)])],
        vec![make("Z", vec![stat("A", f64::NAN, None)])],
        vec![make("Z", vec![stat("A", 0.0, Some(f64::INFINITY))])],
        vec![make("Z", vec![stat("A", -1_000_000_001.0, None)])],
        vec![make("Z", vec![stat(" ", 1.0, None)])],
        vec![make("Z ", vec![stat("A", 1.0, None)])],
        (0..129).map(|i| make(&format!("group-{i}"), vec![])).collect(),
        vec![make("Z", (0..129).map(|i| stat(&format!("stat-{i}"), 1.0, None)).collect())],
        (0..3)
            .map(|i| {
                make(
                    &format!("group-{i}"),
                    (0..86).map(|n| stat(&format!("stat-{n}"), 1.0, None)).collect(),
                )
            })
            .collect(),
    ] {
        assert_eq!(
            ListingEquipmentDetails::new(now(), equipment_label(), bad),
            Err(LookupPermanentReason::SourceSchemaInvalid)
        );
    }
}

#[test]
fn equipment_details_reject_unsupported_label_combinations() {
    for (level, tradeable) in [
        (ListingEquipmentLevel::Soulbound, false),
        (ListingEquipmentLevel::Assigned(1), true),
        (ListingEquipmentLevel::Unassigned, true),
    ] {
        let mut label = equipment_label();
        label.level = level;
        label.tradeable = tradeable;
        let details = ListingEquipmentDetails::new(now(), label, vec![]).fixture();
        assert_eq!(details.label().level, level);
        assert_eq!(details.label().tradeable, tradeable);
    }
    let bad = [
        ListingEquipmentLabel { level: ListingEquipmentLevel::Soulbound, ..equipment_label() },
        ListingEquipmentLabel { level: ListingEquipmentLevel::Assigned(0), ..equipment_label() },
        ListingEquipmentLabel {
            level: ListingEquipmentLevel::Assigned(100_001),
            ..equipment_label()
        },
        ListingEquipmentLabel { name: "a".repeat(513), ..equipment_label() },
        ListingEquipmentLabel { category: "a".repeat(129), ..equipment_label() },
        ListingEquipmentLabel { alt_name: Some(String::new()), ..equipment_label() },
    ];
    for label in bad {
        assert_eq!(
            ListingEquipmentDetails::new(now(), label, vec![]),
            Err(LookupPermanentReason::SourceSchemaInvalid)
        );
    }
}

#[test]
fn snapshots_reject_repeated_credentials_and_hide_owner_in_debug() {
    let owner = "测试持有人";
    let mut key = EquipmentKey::new(FAKE_EQUIPMENT_KEY).fixture();
    let details = ListingEquipmentDetails::new(now(), equipment_label(), vec![]).fixture();
    let snapshot =
        ListingLookupSnapshot::new(identity(), details.clone(), owner.to_owned(), &key).fixture();
    assert_eq!(snapshot.owner_name(), owner);
    assert_eq!(snapshot.identity(), &identity());
    assert!(!format!("{snapshot:?}").contains(owner));
    for owner in [" ", FAKE_EQUIPMENT_KEY] {
        assert_eq!(
            ListingLookupSnapshot::new(identity(), details.clone(), owner.to_owned(), &key),
            Err(LookupPermanentReason::SourceSchemaInvalid.into())
        );
    }
    let suspicious = ListingEquipmentLabel {
        alt_name: Some(FAKE_EQUIPMENT_KEY.to_owned()),
        ..equipment_label()
    };
    let suspicious_details = ListingEquipmentDetails::new(now(), suspicious, vec![]).fixture();
    assert!(!format!("{suspicious_details:?}").contains(FAKE_EQUIPMENT_KEY));
    assert_eq!(
        ListingLookupSnapshot::new(identity(), suspicious_details, owner.to_owned(), &key),
        Err(LookupPermanentReason::SourceSchemaInvalid.into())
    );
    let link = ListingEquipmentLabel {
        name: format!("https://example.invalid/equip/42/{FAKE_EQUIPMENT_KEY}"),
        ..equipment_label()
    };
    assert_eq!(
        ListingEquipmentDetails::new(now(), link, vec![]),
        Err(LookupPermanentReason::SourceSchemaInvalid)
    );
    key.clear();
    assert_eq!(
        ListingLookupSnapshot::new(identity(), details, owner.to_owned(), &key),
        Err(ListingEquipmentLookupError::CredentialUnavailable)
    );
}

#[test]
fn upstream_failures_keep_permanent_progress_and_cooldown_types_separate() {
    use LookupPermanentReason as Permanent;

    assert_eq!(ListingEquipmentLookupError::CredentialUnavailable.transient_reason(), None);

    for (reason, code) in [
        (Permanent::EquipmentNotFound, "equipment_not_found"),
        (Permanent::SourceRequestRejected, "source_request_rejected"),
        (Permanent::SourceHttpInvalid, "source_http_invalid"),
        (Permanent::SourceSchemaInvalid, "source_schema_invalid"),
        (Permanent::SourceTooLarge, "source_too_large"),
        (Permanent::SourceShapeUnconfirmed, "source_shape_unconfirmed"),
        (Permanent::SourceIdentityMismatch, "source_identity_mismatch"),
        (Permanent::OwnerMissing, "owner_missing"),
    ] {
        assert_eq!(ListingReason::from(reason).as_str(), code);
        assert_eq!(ListingEquipmentLookupError::from(reason).transient_reason(), None);
    }
    assert_eq!(LookupRetryAfter::new(Duration::from_secs(12)).duration(), Duration::from_secs(12));
    assert_eq!(
        LookupRetryAfter::new(Duration::from_secs(24 * 3600)).duration(),
        Duration::from_secs(6 * 3600)
    );
    for (error, code) in [
        (ListingEquipmentLookupError::SourceUnavailable, "source_unavailable"),
        (ListingEquipmentLookupError::SourceTimeout, "source_timeout"),
        (ListingEquipmentLookupError::SourceRateLimited(None), "source_rate_limited"),
    ] {
        assert_eq!(error.transient_reason().fixture().as_str(), code);
    }
}

#[test]
fn wrapped_credentials_check_shape_compare_and_redact() {
    let data_key_id = "fake-equipment-data-key";
    let envelope = |fingerprint: String| {
        EquipmentKeyEnvelope::new(vec![4; 20], vec![1; 12], data_key_id.to_owned(), fingerprint)
            .fixture()
    };
    let aad_equipment_id = EquipmentId::from_uuid(Uuid::from_u128(4));
    let secret =
        ListingRegistrationSecret::new(aad_equipment_id, envelope("ad".repeat(32))).fixture();
    assert_eq!(secret.aad_equipment_id(), aad_equipment_id);
    assert_eq!(secret.envelope().ciphertext(), &[4; 20]);
    assert!(!format!("{secret:?}").contains(data_key_id));
    assert!(
        ListingRegistrationSecret::new(
            EquipmentId::from_uuid(Uuid::nil()),
            envelope("ad".repeat(32))
        )
        .is_err()
    );
    for fingerprint in ["ab".repeat(31), "AB".repeat(32), "jk".repeat(32)] {
        assert!(ListingRegistrationSecret::new(aad_equipment_id, envelope(fingerprint)).is_err());
    }
    let wrapped =
        WrappedRegistrationReplayKey::new(vec![5; 48], [4; 12], data_key_id.to_owned()).fixture();
    assert_eq!(wrapped.wrapped_key(), &[5; 48]);
    assert_eq!(wrapped.nonce(), &[4; 12]);
    assert_eq!(wrapped.encryption_key_id(), data_key_id);
    assert_eq!(format!("{wrapped:?}"), "[REDACTED]");
    assert!(
        WrappedRegistrationReplayKey::new(vec![5; 47], [4; 12], data_key_id.to_owned()).is_err()
    );
    assert!(WrappedRegistrationReplayKey::new(vec![5; 48], [4; 12], " bad".to_owned()).is_err());
    let mac = RegistrationRequestMac::new([41; 32]);
    let mut unequal = [41; 32];
    unequal[31] = 42;
    assert!(mac.matches(&RegistrationRequestMac::new([41; 32])));
    assert!(!mac.matches(&RegistrationRequestMac::new(unequal)));
    assert_eq!(format!("{mac:?}"), "[REDACTED]");
}

#[test]
fn lookup_fence_and_claim_budget_reject_invalid_shapes() {
    let fence = LookupFence::new(WORK_ID, LISTING_ID, 1, 7, FAKE_TOKEN).fixture();
    assert_eq!(
        (fence.work_id(), fence.listing_id(), fence.generation(), fence.fence(), fence.token()),
        (WORK_ID, LISTING_ID, 1, 7, FAKE_TOKEN)
    );
    assert!(!format!("{fence:?}").contains("12345678"));
    for (work_id, listing_id, generation, fence, token) in [
        (Uuid::nil(), LISTING_ID, 1, 1, FAKE_TOKEN),
        (WORK_ID, Uuid::nil(), 1, 1, FAKE_TOKEN),
        (WORK_ID, LISTING_ID, 0, 1, FAKE_TOKEN),
        (WORK_ID, LISTING_ID, ListingVersion::MAX + 1, 1, FAKE_TOKEN),
        (WORK_ID, LISTING_ID, 1, 0, FAKE_TOKEN),
        (WORK_ID, LISTING_ID, 1, -1, FAKE_TOKEN),
        (WORK_ID, LISTING_ID, 1, 1, Uuid::nil()),
    ] {
        assert_eq!(
            LookupFence::new(work_id, listing_id, generation, fence, token),
            Err(ConsignmentError::Unavailable)
        );
    }
    let budget = ListingLookupBudget::new(10, 1000).fixture();
    assert_eq!(
        (budget.timeout(), budget.min_interval()),
        (Duration::from_secs(10), Duration::from_secs(1))
    );
    assert!(ListingLookupBudget::new(1, 250).is_ok());
    assert!(ListingLookupBudget::new(30, 60_000).is_ok());
    for (seconds, millis) in [(0, 250), (31, 250), (1, 249), (1, 60_001)] {
        assert_eq!(ListingLookupBudget::new(seconds, millis), Err(ConsignmentError::Unavailable));
    }
    let valid = LookupClaimRequest::new(1, "test-worker".to_owned(), FAKE_TOKEN, budget).fixture();
    assert_eq!(
        (valid.service_epoch(), valid.worker(), valid.token(), valid.budget()),
        (1, "test-worker", FAKE_TOKEN, budget)
    );
    for (epoch, worker, token) in [
        (0, "test-worker", FAKE_TOKEN),
        (ListingVersion::MAX + 1, "test-worker", FAKE_TOKEN),
        (1, " test-worker", FAKE_TOKEN),
        (1, "test-worker", Uuid::nil()),
    ] {
        assert!(matches!(
            LookupClaimRequest::new(epoch, worker.to_owned(), token, budget),
            Err(ConsignmentError::Unavailable)
        ));
    }
}

#[test]
fn effective_view_uses_current_facts_independent_of_committed_state() {
    let details = ListingEquipmentDetails::new(now(), equipment_label(), vec![]).fixture();
    let view = PrivateListingView::Displayed(details.clone());
    assert_eq!(view.effective_state(), (ListingState::Displayed, None));
    let record = PrivateListing {
        listing_id: LISTING_ID,
        version: ListingVersion::new(2).fixture(),
        identity: identity(),
        mode: ListingMode::Direct,
        price: ListingPrice::new(101).fixture(),
        pricing_version: 1,
        fee_bps: FeeBps::DIRECT,
        created_at: now(),
        updated_at: now(),
        expires_at: now(),
        stored_state: ListingState::Displayed,
        view: PrivateListingView::Closed {
            reason: ListingReason::AuctionLotCreated,
            equipment: Some(details),
        },
    };
    assert_eq!(record.stored_state, ListingState::Displayed);
    assert_eq!(record.version.value(), 2);
    assert_eq!(
        record.view.effective_state(),
        (ListingState::Closed, Some(ListingReason::AuctionLotCreated))
    );
}

fn finish<F: Future>(future: F) -> F::Output {
    match std::pin::pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("脚本端口必须立即返回"),
    }
}

#[test]
fn scripted_ports_report_calls_and_preserve_unknown_outcomes() {
    let commands = FakeListingCommands::default();
    let reads = FakeListingReads::default();
    let work = FakeLookupWork::default();
    let equipment = FakeListingEquipment::default();
    let command_id = CommandId::from_uuid(Uuid::from_u128(11));
    let seller_user_id = UserId::from_uuid(Uuid::from_u128(10));
    let price = ListingPrice::new(101).fixture();
    equipment.url.reply(Ok(ListingEquipmentReference {
        identity: identity(),
        key: EquipmentKey::new(FAKE_EQUIPMENT_KEY).fixture(),
    }));
    let reference = equipment.validate("https://example.invalid/fixture").fixture();
    let registration = || RegisterListingCommand {
        command_id,
        seller_user_id,
        input: RegistrationInput {
            identity: &reference.identity,
            key: &reference.key,
            mode: ListingMode::Direct,
            price,
            expected_fee_bps: FeeBps::DIRECT,
        },
        listing_id: LISTING_ID,
        work_id: WORK_ID,
        aad_equipment_id: EquipmentId::from_uuid(Uuid::from_u128(4)),
        candidate_replay_key_id: Uuid::from_u128(5),
        policy: ListingRegistrationPolicy {
            enabled: true,
            worlds: ListingWorlds { main: true, isekai: true },
            consigned_fee_bps: FeeBps::DEFAULT_CONSIGNED,
            max_open_per_user: 20,
            registrations_per_hour: 10,
        },
    };
    commands.register.reply(Err(ConsignmentError::OutcomeUnknown));
    commands.register.reply(Ok(ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(
        ListingRegistered { listing_id: LISTING_ID, price },
    ))));
    assert_eq!(
        finish(commands.register(registration(), Some(&equipment), Some(&equipment))),
        Err(ConsignmentError::OutcomeUnknown)
    );
    let replayed = finish(commands.register(registration(), None, Some(&equipment))).fixture();
    assert_eq!(
        replayed,
        ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(ListingRegistered {
            listing_id: LISTING_ID,
            price
        }))
    );
    let calls = commands.register.take_calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].command_id, calls[1].command_id);
    assert_eq!(calls[1].seller_user_id, seller_user_id);
    assert!(calls[0].sealer);
    assert!(!calls[1].sealer);
    assert!(calls[1].replay_crypto);
    assert!(!format!("{calls:?}").contains(FAKE_EQUIPMENT_KEY));
    assert!(!format!("{:?}", registration()).contains(FAKE_EQUIPMENT_KEY));
    reads.owned_command.reply(Ok(None));
    assert_eq!(finish(reads.owned_command(seller_user_id, command_id)), Ok(None));
    assert_eq!(reads.owned_command.take_calls(), vec![(seller_user_id, command_id)]);
    equipment.lookup.reply(Err(ListingEquipmentLookupError::SourceTimeout));
    assert_eq!(
        finish(equipment.lookup(&reference.identity, &reference.key, now())),
        Err(ListingEquipmentLookupError::SourceTimeout)
    );
    assert_eq!(equipment.lookup.take_calls(), vec![(identity(), now())]);
    work.sync_admission.reply(Ok(ListingAdmission { service_epoch: 2, admitting: false }));
    assert_eq!(
        finish(work.sync_admission(false)),
        Ok(ListingAdmission { service_epoch: 2, admitting: false })
    );
    assert_eq!(work.sync_admission.take_calls(), vec![false]);
}
