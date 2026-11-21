//! 单轮解析的调用顺序、观察原样提交和取消栅栏。

use std::future::{Future, pending, poll_fn, ready};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use std::time::Duration;

use auction_domain::consignment::{
    ListingEquipmentIdentity, ListingMode, ListingPublicationTtls, ListingReason, ListingSnapshot,
    ListingState,
};
use auction_domain::{EquipmentId, EquipmentKey, EquipmentKeyEnvelope};
use auction_time::Timestamp;
use uuid::{Uuid, uuid};

use crate::ports::consignment::test_support::{
    FakeListingCommands, FakeListingEquipment, FakeLookupWork,
};
use crate::ports::consignment::{
    ConsignmentError, ListingCommandOutcome, ListingCommandRejection, ListingCredentialError,
    ListingEquipmentDetails, ListingEquipmentLabel, ListingEquipmentLevel, ListingEquipmentLookup,
    ListingEquipmentLookupError, ListingEquipmentStat, ListingEquipmentStatGroup,
    ListingLookupBudget, ListingLookupClaim, ListingLookupEffect, ListingLookupSnapshot,
    ListingRegistrationSecret, LookupAbandonment, LookupClaimRequest, LookupDispatch, LookupFence,
    LookupLeaseUpdate, LookupObservation, LookupPermanentReason, LookupPermitStatus,
    LookupRetryAfter, LookupTransientReason, StoredListingDecision, UnrecordedListingRejection,
};
use crate::ports::core::{Clock, TransactionFuture};
use crate::services::consignment::encoding::lookup_command_identity;
use crate::services::consignment::tests::{
    FAKE_KEY, LISTING_ID, WORK_ID, identity, price, run, time, version,
};
use crate::services::consignment::{ListingLookupRound, ResolveListingService};

const TOKEN: Uuid = Uuid::from_u128(47);

struct FixedClock(Timestamp);

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

#[derive(Default)]
struct ResolutionFixture {
    work: Arc<FakeLookupWork>,
    commands: Arc<FakeListingCommands>,
    equipment: Arc<FakeListingEquipment>,
}

impl ResolutionFixture {
    fn service(&self) -> ResolveListingService {
        self.service_with(self.equipment.clone(), || false, 2)
    }

    fn service_with(
        &self,
        lookup: Arc<dyn ListingEquipmentLookup>,
        pause: impl Fn() -> bool + Send + Sync + 'static,
        now: i64,
    ) -> ResolveListingService {
        ResolveListingService::new(
            self.work.clone(),
            self.commands.clone(),
            self.equipment.clone(),
            lookup,
            pause,
            Arc::new(FixedClock(time(now))),
            ListingPublicationTtls::new(37, 9).unwrap(),
        )
    }

    fn run(&self) -> Result<ListingLookupRound, ConsignmentError> {
        run(self.service().run_once(request(), pending()))
    }

    fn prepare_dispatch(&self, dispatch: LookupDispatch) {
        self.work.claim.reply(Ok(Some(claim(3, credential()))));
        self.equipment.open.reply(Ok(key()));
        self.work.renew.reply(Ok(LookupLeaseUpdate::Applied(time(16))));
        self.work.confirm_dispatch.reply(Ok(dispatch));
    }

    fn assert_no_network_or_apply(&self) {
        assert!(self.equipment.lookup.take_calls().is_empty());
        assert!(self.commands.apply_lookup.take_calls().is_empty());
    }
}

fn aad_id() -> EquipmentId {
    EquipmentId::from_uuid(Uuid::from_u128(46))
}

fn key() -> EquipmentKey {
    EquipmentKey::new(FAKE_KEY).unwrap()
}

fn request() -> LookupClaimRequest {
    LookupClaimRequest::new(
        3,
        "测试 Worker".to_owned(),
        TOKEN,
        ListingLookupBudget::new(10, 1000).unwrap(),
    )
    .unwrap()
}

fn fence() -> LookupFence {
    LookupFence::new(WORK_ID, LISTING_ID, 1, 2, TOKEN).unwrap()
}

fn credential() -> Result<ListingRegistrationSecret, ListingCredentialError> {
    ListingRegistrationSecret::new(
        aad_id(),
        EquipmentKeyEnvelope::new(
            vec![4; 20],
            vec![1; 12],
            "test-data-key".to_owned(),
            "ab".repeat(32),
        )
        .unwrap(),
    )
}

fn claim(
    epoch: u64,
    credential: Result<ListingRegistrationSecret, ListingCredentialError>,
) -> ListingLookupClaim {
    ListingLookupClaim::new(fence(), identity(), epoch, 2, time(10), time(15), credential).unwrap()
}

fn snapshot(observed_at: i64, owner: &str, tradeable: bool) -> ListingLookupSnapshot {
    snapshot_with_stats(observed_at, owner, tradeable, vec![])
}

fn snapshot_with_stats(
    observed_at: i64,
    owner: &str,
    tradeable: bool,
    stat_groups: Vec<ListingEquipmentStatGroup>,
) -> ListingLookupSnapshot {
    let details = ListingEquipmentDetails::new(
        time(observed_at),
        ListingEquipmentLabel {
            name: "测试装备".to_owned(),
            alt_name: None,
            category: "测试类别".to_owned(),
            level: if tradeable {
                ListingEquipmentLevel::Unassigned
            } else {
                ListingEquipmentLevel::Soulbound
            },
            tradeable,
        },
        stat_groups,
    )
    .unwrap();
    ListingLookupSnapshot::new(identity(), details, owner.to_owned(), &key()).unwrap()
}

fn changed(state: ListingState, reason: Option<ListingReason>) -> ListingLookupEffect {
    ListingLookupEffect::ListingChanged(ListingSnapshot {
        mode: ListingMode::Direct,
        state,
        state_version: version(2),
        price: price(201),
        expires_at: time(100),
        reason,
    })
}

#[test]
fn paused_round_never_claims_or_opens() {
    let fixture = ResolutionFixture::default();
    let service = fixture.service_with(fixture.equipment.clone(), || true, 2);
    assert_eq!(run(service.run_once(request(), pending())), Ok(ListingLookupRound::Paused));
    assert!(fixture.work.claim.take_calls().is_empty());
    assert!(fixture.work.permit_status.take_calls().is_empty());
    assert!(fixture.equipment.open.take_calls().is_empty());
    fixture.assert_no_network_or_apply();
}

#[test]
fn idle_round_uses_latest_persistent_permit_bound_and_handles_new_pause() {
    let fixture = ResolutionFixture::default();
    let status = LookupPermitStatus {
        as_of: time(10),
        service_epoch: 3,
        admitting: true,
        next_allowed_at: time(12),
        permit_until: Some(time(14)),
        cooldown_until: Some(time(20)),
    };
    for (status, wake_at) in
        [(status, Some(time(20))), (LookupPermitStatus { as_of: time(20), ..status }, None)]
    {
        fixture.work.claim.reply(Ok(None));
        fixture.work.permit_status.reply(Ok(status));
        assert_eq!(fixture.run(), Ok(ListingLookupRound::Idle { wake_at }));
    }
    assert!(fixture.work.claim.take_calls().iter().all(|asked| asked == &request()));
    assert_eq!(fixture.work.permit_status.take_calls().len(), 2);
    fixture.work.set_external_access_paused(true);
    let calls = AtomicUsize::new(0);
    let service = fixture.service_with(
        fixture.equipment.clone(),
        move || calls.fetch_add(1, Ordering::Relaxed) > 0,
        2,
    );
    assert_eq!(run(service.run_once(request(), pending())), Ok(ListingLookupRound::Paused));
    assert_eq!(fixture.work.claim.take_calls().len(), 1);
    assert!(fixture.work.permit_status.take_calls().is_empty());
    assert!(fixture.equipment.open.take_calls().is_empty());
    fixture.assert_no_network_or_apply();
}

#[test]
fn missing_or_unopenable_credential_is_applied_without_dispatch() {
    let fixture = ResolutionFixture::default();
    let effect = changed(ListingState::Rejected, Some(ListingReason::CredentialUnavailable));
    for secret in [Err(ListingCredentialError), credential()] {
        fixture.work.claim.reply(Ok(Some(claim(3, secret))));
        fixture.commands.apply_lookup.reply(Ok(ListingCommandOutcome::Accepted(effect)));
    }
    fixture.equipment.open.reply(Err(ListingCredentialError));
    assert_eq!(fixture.run(), Ok(ListingLookupRound::Applied(effect)));
    assert_eq!(fixture.run(), Ok(ListingLookupRound::Applied(effect)));
    assert_eq!(fixture.equipment.open.take_calls(), [aad_id()]);
    assert!(fixture.work.renew.take_calls().is_empty());
    assert!(fixture.work.confirm_dispatch.take_calls().is_empty());
    assert!(fixture.equipment.lookup.take_calls().is_empty());
    let [missing, failed_open] = fixture.commands.apply_lookup.take_calls().try_into().unwrap();
    for call in [&missing, &failed_open] {
        assert!(matches!(
            call.observation,
            LookupObservation::Failed(ListingEquipmentLookupError::CredentialUnavailable)
        ));
        assert_eq!(call.fence, fence());
    }
    assert_eq!(missing.command_id, failed_open.command_id);
    assert_eq!(missing.request_hash, failed_open.request_hash);
}

#[test]
fn dispatch_denial_pause_epoch_and_deadline_never_send() {
    let fixture = ResolutionFixture::default();
    for (dispatch, now, expected) in [
        (LookupDispatch::Lost, 2, ListingLookupRound::LeaseLost),
        (LookupDispatch::Paused, 2, ListingLookupRound::Paused),
        (LookupDispatch::Allowed { deadline: time(8) }, 8, ListingLookupRound::LeaseLost),
        (LookupDispatch::Allowed { deadline: time(16) }, 10, ListingLookupRound::LeaseLost),
    ] {
        fixture.prepare_dispatch(dispatch);
        fixture.work.abandon.reply(Ok(LookupLeaseUpdate::Applied(())));
        let service = fixture.service_with(fixture.equipment.clone(), || false, now);
        assert_eq!(run(service.run_once(request(), pending())), Ok(expected));
    }
    let paused_calls = AtomicUsize::new(0);
    let service = fixture.service_with(
        fixture.equipment.clone(),
        move || paused_calls.fetch_add(1, Ordering::Relaxed) > 0,
        2,
    );
    fixture.prepare_dispatch(LookupDispatch::Allowed { deadline: time(7) });
    fixture.work.abandon.reply(Ok(LookupLeaseUpdate::Lost));
    assert_eq!(run(service.run_once(request(), pending())), Ok(ListingLookupRound::Paused));
    let paused_after_confirm = AtomicUsize::new(0);
    let service = fixture.service_with(
        fixture.equipment.clone(),
        move || paused_after_confirm.fetch_add(1, Ordering::Relaxed) > 1,
        2,
    );
    fixture.prepare_dispatch(LookupDispatch::Allowed { deadline: time(7) });
    fixture.work.abandon.reply(Ok(LookupLeaseUpdate::Applied(())));
    assert_eq!(run(service.run_once(request(), pending())), Ok(ListingLookupRound::Paused));

    fixture.work.claim.reply(Ok(Some(claim(4, credential()))));
    assert_eq!(fixture.run(), Ok(ListingLookupRound::LeaseLost));
    let wrong_token = LookupClaimRequest::new(
        3,
        "测试 Worker".to_owned(),
        Uuid::from_u128(99),
        request().budget(),
    )
    .unwrap();
    fixture.work.claim.reply(Ok(Some(claim(3, credential()))));
    assert_eq!(
        run(fixture.service().run_once(wrong_token, pending())),
        Ok(ListingLookupRound::LeaseLost)
    );
    assert_eq!(fixture.work.abandon.take_calls(), [(fence(), LookupAbandonment::NotSent); 6]);
    assert_eq!(fixture.equipment.open.take_calls(), [aad_id(); 6]);
    assert_eq!(
        fixture.work.confirm_dispatch.take_calls(),
        [
            (fence(), false),
            (fence(), false),
            (fence(), false),
            (fence(), false),
            (fence(), true),
            (fence(), false),
        ]
    );
    fixture.assert_no_network_or_apply();
}

#[test]
fn lost_or_failed_renewal_never_confirms_or_sends() {
    let fixture = ResolutionFixture::default();
    for (renewal, expected) in [
        (Ok(LookupLeaseUpdate::Lost), Ok(ListingLookupRound::LeaseLost)),
        (Err(ConsignmentError::Unavailable), Err(ConsignmentError::Unavailable)),
    ] {
        fixture.work.claim.reply(Ok(Some(claim(3, credential()))));
        fixture.equipment.open.reply(Ok(key()));
        fixture.work.renew.reply(renewal);
        assert_eq!(fixture.run(), expected);
    }
    assert_eq!(fixture.work.renew.take_calls(), [fence(); 2]);
    assert_eq!(fixture.equipment.open.take_calls(), [aad_id(); 2]);
    assert!(fixture.work.confirm_dispatch.take_calls().is_empty());
    assert!(fixture.work.abandon.take_calls().is_empty());
    fixture.assert_no_network_or_apply();
}

#[test]
fn permanent_transient_and_429_observations_are_forwarded_unchanged() {
    use LookupPermanentReason::{
        EquipmentNotFound, OwnerMissing, SourceHttpInvalid, SourceIdentityMismatch,
        SourceRequestRejected, SourceSchemaInvalid, SourceShapeUnconfirmed, SourceTooLarge,
    };
    let fixture = ResolutionFixture::default();
    let classifications = [
        (EquipmentNotFound, ListingReason::EquipmentNotFound),
        (SourceRequestRejected, ListingReason::SourceRequestRejected),
        (SourceHttpInvalid, ListingReason::SourceHttpInvalid),
        (SourceSchemaInvalid, ListingReason::SourceSchemaInvalid),
        (SourceTooLarge, ListingReason::SourceTooLarge),
        (SourceShapeUnconfirmed, ListingReason::SourceShapeUnconfirmed),
        (SourceIdentityMismatch, ListingReason::SourceIdentityMismatch),
        (OwnerMissing, ListingReason::OwnerMissing),
    ];
    let transient = [
        (ListingEquipmentLookupError::SourceUnavailable, LookupTransientReason::SourceUnavailable),
        (ListingEquipmentLookupError::SourceTimeout, LookupTransientReason::SourceTimeout),
        (
            ListingEquipmentLookupError::SourceRateLimited(None),
            LookupTransientReason::SourceRateLimited,
        ),
        (
            ListingEquipmentLookupError::SourceRateLimited(Some(LookupRetryAfter::new(
                Duration::from_secs(21_600),
            ))),
            LookupTransientReason::SourceRateLimited,
        ),
    ];
    let cases = classifications
        .map(|(error, reason)| {
            (
                ListingEquipmentLookupError::Permanent(error),
                changed(ListingState::Rejected, Some(reason)),
            )
        })
        .into_iter()
        .chain(transient.map(|(error, reason)| {
            (error, ListingLookupEffect::Rescheduled { reason, next_attempt_at: time(30) })
        }));
    for (error, effect) in cases {
        fixture.prepare_dispatch(LookupDispatch::Allowed { deadline: time(7) });
        fixture.equipment.lookup.reply(Err(error));
        fixture.commands.apply_lookup.reply(Ok(ListingCommandOutcome::Accepted(effect)));
        assert_eq!(fixture.run(), Ok(ListingLookupRound::Applied(effect)));
        assert_eq!(fixture.equipment.lookup.take_calls(), [(identity(), time(7))]);
        let [applied] = fixture.commands.apply_lookup.take_calls().try_into().unwrap();
        assert!(
            matches!(applied.observation, LookupObservation::Failed(observed) if observed == error)
        );
        assert_eq!(applied.publication_ttls, ListingPublicationTtls::new(37, 9).unwrap());
        assert_eq!(applied.fence, fence());
    }
    assert_eq!(fixture.equipment.open.take_calls(), [aad_id(); 12]);
    assert_eq!(fixture.work.confirm_dispatch.take_calls(), [(fence(), false); 12]);
    assert!(fixture.work.abandon.take_calls().is_empty());
}

#[test]
fn last_attempt_forwards_429_for_cooldown_even_when_the_work_is_exhausted() {
    let fixture = ResolutionFixture::default();
    fixture.work.claim.reply(Ok(Some(
        ListingLookupClaim::new(fence(), identity(), 3, 6, time(10), time(15), credential())
            .unwrap(),
    )));
    fixture.equipment.open.reply(Ok(key()));
    fixture.work.renew.reply(Ok(LookupLeaseUpdate::Applied(time(15))));
    fixture.work.confirm_dispatch.reply(Ok(LookupDispatch::Allowed { deadline: time(7) }));
    let limited = ListingEquipmentLookupError::SourceRateLimited(Some(LookupRetryAfter::new(
        Duration::from_secs(21_600),
    )));
    fixture.equipment.lookup.reply(Err(limited));
    let exhausted = changed(ListingState::Rejected, Some(ListingReason::LookupExhausted));
    fixture.commands.apply_lookup.reply(Ok(ListingCommandOutcome::Accepted(exhausted)));

    assert_eq!(fixture.run(), Ok(ListingLookupRound::Applied(exhausted)));
    assert_eq!(fixture.equipment.lookup.take_calls(), [(identity(), time(7))]);
    let [call] = fixture.commands.apply_lookup.take_calls().try_into().unwrap();
    assert!(matches!(call.observation, LookupObservation::Failed(observed) if observed == limited));
    assert_eq!(call.command_id.as_uuid(), uuid!("286fc89a-66a7-5282-a414-518a35f2fe82"));
    assert_eq!(
        call.request_hash.as_bytes(),
        [
            253, 231, 10, 123, 91, 51, 38, 1, 113, 12, 26, 157, 6, 238, 115, 123, 125, 188, 150,
            71, 174, 243, 84, 40, 219, 163, 21, 46, 63, 103, 146, 125
        ],
    );
    assert!(fixture.work.abandon.take_calls().is_empty());
}

#[test]
fn full_observation_is_forwarded_and_only_store_decides_qualification() {
    let fixture = ResolutionFixture::default();
    let original = snapshot(3, "测试持有人", true);
    let time_changed = snapshot(4, "测试持有人", true);
    let owner_changed = snapshot(4, "另一持有人", true);
    let soulbound = snapshot(4, "测试持有人", false);
    let published = changed(ListingState::Displayed, None);
    let owner_rejected = changed(ListingState::Rejected, Some(ListingReason::OwnerMismatch));
    let soulbound_rejected = changed(ListingState::Rejected, Some(ListingReason::NotTradeable));
    let mut commands = Vec::new();
    for (snapshot, effect, outcome) in [
        (original, published, ListingCommandOutcome::Accepted(published)),
        (
            time_changed,
            published,
            ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(published)),
        ),
        (owner_changed, owner_rejected, ListingCommandOutcome::Accepted(owner_rejected)),
        (soulbound, soulbound_rejected, ListingCommandOutcome::Accepted(soulbound_rejected)),
    ] {
        fixture.prepare_dispatch(LookupDispatch::Allowed { deadline: time(16) });
        fixture.equipment.lookup.reply(Ok(snapshot.clone()));
        fixture.commands.apply_lookup.reply(Ok(outcome));
        assert_eq!(fixture.run(), Ok(ListingLookupRound::Applied(effect)));
        let calls = fixture.equipment.lookup.take_calls();
        assert_eq!(calls, [(identity(), time(10))]);
        // 2P 的替身保留完整观察，应用不得丢弃采集时间或自行筛掉所有者、魂绑分支。
        let [call] = fixture.commands.apply_lookup.take_calls().try_into().unwrap();
        assert!(
            matches!(call.observation, LookupObservation::Equipment(ref value) if value == &snapshot)
        );
        commands.push((call.command_id, call.request_hash));
    }
    let [first, replay, owner, soulbound] = commands.try_into().unwrap();
    assert_eq!(first.0, replay.0);
    assert_eq!(first.0, owner.0);
    assert_eq!(first.0, soulbound.0);
    assert_eq!(first.1, replay.1);
    assert_ne!(first.1, owner.1);
    assert_ne!(first.1, soulbound.1);
}

#[test]
fn apply_lease_loss_and_conflict_do_not_retry_or_release_later_permit() {
    let fixture = ResolutionFixture::default();
    for (reply, expected) in [
        (
            Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Unrecorded(
                UnrecordedListingRejection::LeaseLost,
            ))),
            Ok(ListingLookupRound::LeaseLost),
        ),
        (
            Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Unrecorded(
                UnrecordedListingRejection::CommandIdConflict,
            ))),
            Err(ConsignmentError::CommandIdConflict),
        ),
    ] {
        fixture.prepare_dispatch(LookupDispatch::Allowed { deadline: time(7) });
        fixture.equipment.lookup.reply(Err(ListingEquipmentLookupError::SourceTimeout));
        fixture.commands.apply_lookup.reply(reply);
        assert_eq!(fixture.run(), expected);
    }
    assert_eq!(fixture.equipment.lookup.take_calls().len(), 2);
    assert_eq!(fixture.commands.apply_lookup.take_calls().len(), 2);
    assert!(fixture.work.abandon.take_calls().is_empty());
}

#[test]
fn lookup_stats_change_digest_without_changing_fenced_command_id() {
    let observation = |value, base| {
        LookupObservation::Equipment(snapshot_with_stats(
            3,
            "测试持有人",
            true,
            vec![ListingEquipmentStatGroup {
                name: "测试属性组".to_owned(),
                stats: vec![ListingEquipmentStat {
                    name: "测试属性".to_owned(),
                    value,
                    base: Some(base),
                }],
            }],
        ))
    };
    let (command_id, original) = lookup_command_identity(fence(), &observation(12.5, 10.0));
    let (value_id, changed_value) = lookup_command_identity(fence(), &observation(13.5, 10.0));
    let (base_id, changed_base) = lookup_command_identity(fence(), &observation(12.5, 11.0));
    assert_eq!(value_id, command_id);
    assert_eq!(base_id, command_id);
    assert_ne!(changed_value, original, "value 参与摘要");
    assert_ne!(changed_base, original, "base 参与摘要");
}

#[test]
fn unknown_lookup_commit_retries_the_exact_observation_once_without_network() {
    let fixture = ResolutionFixture::default();
    let observation = snapshot(3, "测试持有人", true);
    let effect = changed(ListingState::Displayed, None);
    for (second_reply, expected) in [
        (
            Ok(ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(effect))),
            Ok(ListingLookupRound::Applied(effect)),
        ),
        (Err(ConsignmentError::Unavailable), Err(ConsignmentError::OutcomeUnknown)),
        (Err(ConsignmentError::OutcomeUnknown), Err(ConsignmentError::OutcomeUnknown)),
    ] {
        fixture.prepare_dispatch(LookupDispatch::Allowed { deadline: time(7) });
        fixture.equipment.lookup.reply(Ok(observation.clone()));
        fixture.commands.apply_lookup.reply(Err(ConsignmentError::OutcomeUnknown));
        fixture.commands.apply_lookup.reply(second_reply);
        assert_eq!(fixture.run(), expected);
        let [first, retry] = fixture.commands.apply_lookup.take_calls().try_into().unwrap();
        assert_eq!(first.command_id, retry.command_id);
        assert_eq!(first.fence, retry.fence);
        assert_eq!(first.request_hash, retry.request_hash);
        assert_eq!(first.publication_ttls, retry.publication_ttls);
        for submitted in [first, retry] {
            assert!(
                matches!(submitted.observation, LookupObservation::Equipment(value) if value == observation)
            );
        }
        assert_eq!(fixture.equipment.lookup.take_calls(), [(identity(), time(7))]);
    }
    assert!(fixture.work.abandon.take_calls().is_empty());
}

struct PendingEquipment(Arc<FakeListingEquipment>);

impl ListingEquipmentLookup for PendingEquipment {
    fn lookup<'a>(
        &'a self,
        identity: &'a ListingEquipmentIdentity,
        key: &'a EquipmentKey,
        deadline: Timestamp,
    ) -> TransactionFuture<'a, ListingLookupSnapshot, ListingEquipmentLookupError> {
        Box::pin(async move {
            self.0.lookup(identity, key, deadline).await?;
            pending().await
        })
    }
}

fn on_second_poll() -> impl Future<Output = ()> {
    let mut polled = false;
    poll_fn(
        move |_| {
            if std::mem::replace(&mut polled, true) { Poll::Ready(()) } else { Poll::Pending }
        },
    )
}

#[test]
fn shutdown_before_send_or_in_flight_uses_the_correct_permit_fence() {
    let fixture = ResolutionFixture::default();
    let service =
        fixture.service_with(Arc::new(PendingEquipment(fixture.equipment.clone())), || false, 2);
    fixture.prepare_dispatch(LookupDispatch::Allowed { deadline: time(7) });
    fixture.work.abandon.reply(Ok(LookupLeaseUpdate::Applied(())));
    assert_eq!(run(service.run_once(request(), ready(()))), Ok(ListingLookupRound::Cancelled));
    fixture.assert_no_network_or_apply();

    fixture.prepare_dispatch(LookupDispatch::Allowed { deadline: time(7) });
    fixture.equipment.lookup.reply(Ok(snapshot(3, "测试持有人", true)));
    fixture.work.abandon.reply(Ok(LookupLeaseUpdate::Applied(())));
    assert_eq!(
        run(service.run_once(request(), on_second_poll())),
        Ok(ListingLookupRound::Cancelled)
    );
    assert_eq!(fixture.equipment.lookup.take_calls(), [(identity(), time(7))]);
    assert_eq!(
        fixture.work.abandon.take_calls(),
        [(fence(), LookupAbandonment::NotSent), (fence(), LookupAbandonment::MayBeInFlight)]
    );
    assert!(fixture.commands.apply_lookup.take_calls().is_empty());
}

#[test]
fn recorded_listing_not_active_ends_lookup_on_first_submit_or_unknown_commit_replay() {
    use crate::ports::consignment::ListingBusinessRejection;

    let fixture = ResolutionFixture::default();
    let reason = ListingBusinessRejection::ListingNotActive;
    for replay in [false, true] {
        fixture.prepare_dispatch(LookupDispatch::Allowed { deadline: time(7) });
        fixture.equipment.lookup.reply(Err(ListingEquipmentLookupError::SourceTimeout));
        if replay {
            fixture.commands.apply_lookup.reply(Err(ConsignmentError::OutcomeUnknown));
            fixture.commands.apply_lookup.reply(Ok(ListingCommandOutcome::Replayed(
                StoredListingDecision::Rejected(reason),
            )));
        } else {
            fixture.commands.apply_lookup.reply(Ok(ListingCommandOutcome::Rejected(
                ListingCommandRejection::Recorded(reason),
            )));
        }
        assert_eq!(fixture.run(), Ok(ListingLookupRound::WorkEnded));
        let calls = fixture.commands.apply_lookup.take_calls();
        assert_eq!(calls.len(), if replay { 2 } else { 1 });
        if replay {
            assert_eq!(calls[0].command_id, calls[1].command_id);
            assert_eq!(calls[0].request_hash, calls[1].request_hash);
        }
        assert_eq!(fixture.equipment.lookup.take_calls().len(), 1);
        assert!(fixture.work.abandon.take_calls().is_empty());
    }
}
