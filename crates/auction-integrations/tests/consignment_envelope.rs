//! 寄售原观察、认证读取、严格处置与封装的离线回归。
//!
//! 页面取自脱敏采集件并派生为两世界寄售形状；派生件未签收，只用于分支测试，不冒充真实样本。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use auction_application::admin::{AdminCredentialRuntimeOwnership, AdminCredentialRuntimeWriter};
use auction_application::consignment::PostPermit;
use auction_application::core::{ApplicationError, ApplicationFuture};
use auction_application::forum::{ForumTargetProfileEvidence, ForumTargetProfileReader};
use auction_application::mail::MailDisplaySnapshot;
use auction_application::mail::{MailPollClaim, MailPollReadJournal, MailPollReadSummary};
use auction_application::settlement::{InternalEvidenceFuture, MailRuntimeBinding};
use auction_domain::SettlementWorld;
use auction_domain::consignment::EnvelopeItemKind;
use auction_integrations::mooglemail::{
    CONSIGNMENT_DISPOSITION_VALIDATOR_VERSION, CONSIGNMENT_PARSER_VERSION, ConsignmentReadOutcome,
    ConsignmentReadPending, DispositionAction, DispositionOutcome, DispositionRequest,
    DispositionTarget, ExpectedReturn, MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie,
    MoogleMailCredentialSet, MoogleMailDeliveryTransport, MoogleMailGateway,
    MoogleMailGatewayConfig, MoogleMailParser, MoogleMailRuntimeCredential, MoogleMailWorld,
    NotSentReason, SentUnprovenReason, parse_consignment_detail, parse_consignment_list_row,
};
use auction_test_support::mooglemail::{Fault, MailDouble};
use auction_time::Timestamp;
use uuid::Uuid;

struct NoInvalidation;

impl AdminCredentialRuntimeWriter for NoInvalidation {
    fn mark_invalid(&self, _credential_id: Uuid, _now: Timestamp) -> ApplicationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

struct ValidOwner;

impl AdminCredentialRuntimeOwnership for ValidOwner {
    fn check(&self) -> ApplicationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn is_valid(&self) -> bool {
        true
    }

    fn mail_runtime_binding(&self) -> Option<MailRuntimeBinding> {
        Some(MailRuntimeBinding { generation: 1 })
    }

    fn revoke_internal_use(&self) {}
}

struct FixtureProfile {
    identity: Arc<std::sync::Mutex<(String, String)>>,
}

impl FixtureProfile {
    fn new(uid: &str, name: &str) -> Self {
        Self { identity: Arc::new(std::sync::Mutex::new((uid.to_owned(), name.to_owned()))) }
    }

    fn set(&self, uid: &str, name: &str) {
        *self.identity.lock().expect("fixture profile") = (uid.to_owned(), name.to_owned());
    }
}

impl ForumTargetProfileReader for FixtureProfile {
    fn read_target_profile(
        &self,
        target_uid: String,
        _deadline: Instant,
    ) -> InternalEvidenceFuture<'_, ForumTargetProfileEvidence> {
        let identity = Arc::clone(&self.identity);
        Box::pin(async move {
            let (uid, name) = identity.lock().expect("fixture profile").clone();
            if target_uid != uid {
                return Err(auction_application::settlement::InternalEvidenceError::Unproven);
            }
            Ok(ForumTargetProfileEvidence {
                target_uid,
                player_name: name,
                observed_at: Timestamp::now(),
                protocol_version: "forum_target_profile_mid_v1",
                digest: [1; 32],
            })
        })
    }
}

struct NoLoginKeys;

impl auction_application::mail::MailLoginKeyReader for NoLoginKeys {
    fn read(
        &self,
        _user_id: Uuid,
    ) -> ApplicationFuture<'_, auction_application::mail::MailLoginKey> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("login key")) })
    }
}

fn client(server: &MailDouble) -> MoogleMailClient {
    MoogleMailClient::with_config(MoogleMailClientConfig {
        endpoint: server.endpoint(),
        maximum_response_bytes: 8 * 1024 * 1024,
        minimum_request_interval: Duration::from_millis(1),
        request_timeout: Duration::from_secs(2),
    })
    .expect("loopback client")
}

fn credentials(world: MoogleMailWorld) -> MoogleMailCredentialSet {
    MoogleMailCredentialSet::from_runtime_credentials(
        vec![MoogleMailRuntimeCredential {
            credential_id: Uuid::from_u128(101),
            state_version: 1,
            world: Some(world),
            player_name: Some(BOT_NAME.to_owned()),
            cookie: MoogleMailCookie::new(COOKIE).expect("fixture cookie"),
        }],
        Arc::new(NoInvalidation),
        Some(Arc::new(ValidOwner)),
    )
    .expect("persistent fixture credentials")
}

fn gateway(
    server: &MailDouble,
    world: MoogleMailWorld,
    allowlist: &[&str],
    profile: Arc<FixtureProfile>,
) -> (Arc<MoogleMailGateway>, MoogleMailDeliveryTransport) {
    let client = client(server);
    let credentials = credentials(world);
    let gateway = Arc::new(
        MoogleMailGateway::new(
            Arc::new(client.clone()),
            credentials.clone(),
            MoogleMailGatewayConfig {
                maximum_pages: 10,
                sender_allowlist: allowlist.iter().map(|value| (*value).to_owned()).collect(),
                sender_allowlist_disabled: allowlist.is_empty(),
                fetch_budget: Duration::from_secs(3),
            },
        )
        .expect("gateway")
        .with_forum_profile_reader(profile),
    );
    let transport = MoogleMailDeliveryTransport::new(client, credentials, Arc::new(NoLoginKeys));
    (gateway, transport)
}

async fn observe(
    gateway: &MoogleMailGateway,
    row: &auction_integrations::mooglemail::ConsignmentListRow,
) -> auction_integrations::mooglemail::BoundConsignmentObservation {
    match read_envelope(gateway, row).await {
        ConsignmentReadOutcome::Observed(observation) => *observation,
        ConsignmentReadOutcome::Pending(reason) => {
            panic!("expected observation, got pending {reason:?}")
        }
    }
}

struct AcceptingJournal;

impl MailPollReadJournal for AcceptingJournal {
    fn pending_reads(
        &self,
        _claim: MailPollClaim,
        _world_id: String,
        _limit: u16,
    ) -> ApplicationFuture<'_, Vec<MailPollReadSummary>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn stage_reads(
        &self,
        _claim: MailPollClaim,
        _world_id: String,
        _summaries: Vec<MailPollReadSummary>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JournalEvent {
    Pending { requests_before: usize },
    Stage { requests_before_get: usize },
}

struct RecordingJournal {
    server: Arc<MailDouble>,
    stage_fails: bool,
    pending: Vec<MailPollReadSummary>,
    events: Mutex<Vec<JournalEvent>>,
}

impl MailPollReadJournal for RecordingJournal {
    fn pending_reads(
        &self,
        _claim: MailPollClaim,
        _world_id: String,
        _limit: u16,
    ) -> ApplicationFuture<'_, Vec<MailPollReadSummary>> {
        let requests_before = self.server.requests().len();
        self.events.lock().expect("journal events").push(JournalEvent::Pending { requests_before });
        let pending = self.pending.clone();
        Box::pin(async move { Ok(pending) })
    }

    fn stage_reads(
        &self,
        _claim: MailPollClaim,
        _world_id: String,
        _summaries: Vec<MailPollReadSummary>,
    ) -> ApplicationFuture<'_, ()> {
        let requests_before_get = self.server.requests().len();
        self.events
            .lock()
            .expect("journal events")
            .push(JournalEvent::Stage { requests_before_get });
        let stage_fails = self.stage_fails;
        Box::pin(async move {
            if stage_fails {
                Err(ApplicationError::CapabilityUnavailable("fixture journal"))
            } else {
                Ok(())
            }
        })
    }
}

fn poll_claim() -> MailPollClaim {
    MailPollClaim {
        mailbox_key: "fixture-mailbox".to_owned(),
        worker_id: "fixture-worker".to_owned(),
        claim_token: Uuid::new_v4(),
        cursor_value: None,
        state_version: 1,
        paused: false,
        attempt_count: 1,
        lease_until: Timestamp::now() + auction_time::Duration::seconds(60),
    }
}

async fn read_envelope(
    gateway: &MoogleMailGateway,
    row: &auction_integrations::mooglemail::ConsignmentListRow,
) -> ConsignmentReadOutcome {
    gateway
        .read_consignment_envelope(
            row,
            Instant::now() + Duration::from_secs(3),
            &AcceptingJournal,
            &poll_claim(),
        )
        .await
}

struct NeverStores;

impl auction_application::consignment::ReservationStore for NeverStores {
    fn issue_reservation(
        &self,
        _command: auction_application::consignment::IssueReservationCommand,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        auction_application::consignment::ReservationReceipt,
    > {
        Box::pin(async { Err(auction_application::consignment::ConsignmentError::Unavailable) })
    }

    fn expire_reservation(
        &self,
        _command: auction_application::consignment::ExpireReservationCommand,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        auction_application::consignment::ReservationRetired,
    > {
        Box::pin(async { Err(auction_application::consignment::ConsignmentError::Unavailable) })
    }

    fn replay_issue_reservation(
        &self,
        _seller_user_id: auction_domain::UserId,
        _command_id: auction_domain::CommandId,
        _listing_id: Uuid,
        _request_hash: auction_application::core::RequestHash,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        Option<auction_application::consignment::ReservationReceipt>,
    > {
        Box::pin(async { Err(auction_application::consignment::ConsignmentError::Unavailable) })
    }
}

impl auction_application::consignment::EnvelopeStore for NeverStores {
    fn decide_envelope(
        &self,
        _command: auction_application::consignment::DecideEnvelopeCommand,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        auction_application::consignment::EnvelopeReceipt,
    > {
        Box::pin(async { Err(auction_application::consignment::ConsignmentError::Unavailable) })
    }

    fn append_quarantine_observation(
        &self,
        _command: auction_application::consignment::AppendQuarantineObservationCommand,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        auction_application::consignment::ObservationReceipt,
    > {
        Box::pin(async { Err(auction_application::consignment::ConsignmentError::Unavailable) })
    }

    fn recheck_quarantine(
        &self,
        _command: auction_application::consignment::RecheckQuarantineCommand,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        auction_application::consignment::RecheckReceipt,
    > {
        Box::pin(async { Err(auction_application::consignment::ConsignmentError::Unavailable) })
    }

    fn release_quarantine(
        &self,
        _command: auction_application::consignment::ReleaseQuarantineCommand,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        auction_application::consignment::ReleaseReceipt,
    > {
        Box::pin(async { Err(auction_application::consignment::ConsignmentError::Unavailable) })
    }
}

impl auction_application::consignment::CustodyControlStore for NeverStores {
    fn request_withdrawal(
        &self,
        _command: auction_application::consignment::WithdrawDepositCommand,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        auction_application::consignment::WithdrawDepositReceipt,
    > {
        Box::pin(async { Err(auction_application::consignment::ConsignmentError::Unavailable) })
    }

    fn set_operations_hold(
        &self,
        _command: auction_application::consignment::SetOperationsHoldCommand,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        auction_application::consignment::OperationsHoldReceipt,
    > {
        Box::pin(async { Err(auction_application::consignment::ConsignmentError::Unavailable) })
    }

    fn release_operations_hold(
        &self,
        _command: auction_application::consignment::ReleaseOperationsHoldCommand,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        auction_application::consignment::OperationsHoldReceipt,
    > {
        Box::pin(async { Err(auction_application::consignment::ConsignmentError::Unavailable) })
    }

    fn replay_set_operations_hold(
        &self,
        _admin_actor: String,
        _command_id: auction_domain::CommandId,
        _listing_id: Uuid,
        _request_hash: auction_application::core::RequestHash,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        Option<auction_application::consignment::OperationsHoldReceipt>,
    > {
        Box::pin(async { Err(auction_application::consignment::ConsignmentError::Unavailable) })
    }
}

impl auction_application::consignment::SeasonStore for NeverStores {
    fn queue_season_notice(
        &self,
        _command: auction_application::consignment::SeasonNoticeCommand,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        auction_application::consignment::SeasonNoticeReceipt,
    > {
        Box::pin(async { Err(auction_application::consignment::ConsignmentError::Unavailable) })
    }

    fn close_season(
        &self,
        _command: auction_application::consignment::SeasonCloseCommand,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        auction_application::consignment::SeasonCloseReceipt,
    > {
        Box::pin(async { Err(auction_application::consignment::ConsignmentError::Unavailable) })
    }
}

impl auction_application::consignment::DispositionStore for NeverStores {
    fn start_disposition_write(
        &self,
        command: auction_application::consignment::StartDispositionWriteCommand,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        auction_application::consignment::WriteStartOutcome,
    > {
        Box::pin(async move {
            Ok(auction_application::consignment::WriteStartOutcome::FirstCommitted(
                auction_application::consignment::StartReceipt::started(
                    command.command_id,
                    command.final_decision_id,
                    command.work_id,
                    command.attempt_id,
                    command.fence,
                ),
            ))
        })
    }

    fn apply_mail_proof(
        &self,
        command: auction_application::consignment::ApplyMailProofCommand,
    ) -> auction_application::consignment::ConsignmentFuture<
        '_,
        auction_application::consignment::ProofReceipt,
    > {
        Box::pin(async move {
            Ok(auction_application::consignment::ProofReceipt::new(
                command.command_id,
                auction_application::consignment::ProofDisposition::EvidenceQuarantined,
                command.final_decision_id,
                Uuid::new_v4(),
            ))
        })
    }
}

async fn permit_for(
    prepared: &auction_integrations::mooglemail::PreparedConsignmentDisposition,
) -> PostPermit {
    let target = prepared.request().target();
    permit_with(
        target.work_id,
        target.delivery_id,
        target.fence,
        prepared.session_binding_digest(),
        target.frozen.manifest_digest,
    )
    .await
}

async fn permit_with(
    work_id: Uuid,
    delivery_id: Uuid,
    fence: u64,
    session_binding_digest: [u8; 32],
    expected_manifest_digest: [u8; 32],
) -> PostPermit {
    let stores = Arc::new(NeverStores);
    let service = auction_application::consignment::DepositCommandService::new(
        stores.clone(),
        stores.clone(),
        stores.clone(),
        stores.clone(),
        stores,
    );
    let input = auction_application::consignment::StartDispositionWriteInput {
        final_decision_id: Uuid::new_v4(),
        action: auction_application::consignment::DispositionAction::Take,
        delivery_id,
        fence,
        work_id,
        attempt_id: Uuid::new_v4(),
        claim_token: Uuid::new_v4(),
        session_binding_digest,
        expected_manifest_digest,
    };
    match service.start_disposition_write(input).await.expect("fixture ledger") {
        auction_application::consignment::WriteStart::FirstCommitted { permit, .. } => permit,
        auction_application::consignment::WriteStart::AlreadyCommitted(_) => {
            panic!("fixture ledger must issue the first permit")
        }
    }
}

fn target_for(
    observation: &auction_integrations::mooglemail::BoundConsignmentObservation,
    fence: u64,
) -> DispositionTarget {
    DispositionTarget::new(
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        fence,
        observation.frozen_manifest().expect("冻结清单"),
    )
    .expect("合法目标")
}

fn behavior_row(mid: &str) -> auction_integrations::mooglemail::ConsignmentListRow {
    auction_integrations::mooglemail::ConsignmentListRow {
        external_mail_id: mid.to_owned(),
        ..list_row(LIST_MAIN, MoogleMailWorld::Main, "900001")
    }
}

async fn execute_take(mid: &str, before: &str, after: String) -> (MailDouble, DispositionOutcome) {
    let server = MailDouble::start().await.expect("loopback");
    server.script_take_mail(mid, before.to_owned(), after);
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, transport) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let observation = observe(&gateway, &behavior_row(mid)).await;
    let prepared = gateway
        .prepare_consignment_disposition(
            DispositionRequest::Take(target_for(&observation, 3)),
            Instant::now() + Duration::from_secs(3),
        )
        .await
        .expect("处置准备");
    let permit = permit_for(&prepared).await;
    let outcome = transport.execute_consignment_disposition(prepared, permit).await;
    (server, outcome)
}

async fn execute_return(
    mid: &str,
    before: &str,
    after: String,
) -> (MailDouble, DispositionOutcome) {
    let server = MailDouble::start().await.expect("loopback");
    server.script_return_mail(mid, before.to_owned(), after);
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, transport) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let observation = observe(&gateway, &behavior_row(mid)).await;
    let target = target_for(&observation, 3);
    let expected = ExpectedReturn::new(observation.frozen_manifest().expect("冻结清单"));
    let prepared = gateway
        .prepare_consignment_disposition(
            DispositionRequest::Return { target: Box::new(target), expected_return: expected },
            Instant::now() + Duration::from_secs(3),
        )
        .await
        .expect("处置准备");
    let permit = permit_for(&prepared).await;
    let outcome = transport.execute_consignment_disposition(prepared, permit).await;
    (server, outcome)
}

fn take_after_with_attachment() -> String {
    DETAIL_TAKE_AFTER
        .replace("0 / 10 items attached", "1 / 10 items attached")
        .replace(
            "<div id=\"mmail_attachlist\"></div>",
            "<div id=\"mmail_attachlist\"><div><div><div onmouseover=\"equips.set(900001,'mmail_attachlist',340,250)\" onmouseout=\"equips.unset()\">Fixture Blade</div></div><div></div></div></div>",
        )
        .replace(
            "var dynjs_eqstore = {};",
            "var dynjs_eqstore = {\"900001\":{\"t\":\"Fixture Blade\",\"k\":\"0123456789\"}};",
        )
        .replace("var attach_count = 0;", "var attach_count = 1;")
}

#[tokio::test]
async fn authenticated_read_binds_uid_and_full_player_name() {
    let server = MailDouble::start().await.expect("loopback");
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, _transport) =
        gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let observation = observe(&gateway, &row).await;
    assert_eq!(observation.binding().authenticated_uid, BOT_UID);
    assert_eq!(observation.binding().player_name, BOT_NAME);
    assert_eq!(observation.binding().world, SettlementWorld::Main);
    assert_eq!(observation.binding().credential_id, Uuid::from_u128(101));
    assert_eq!(observation.binding().credential_version, 1);
    assert_eq!(observation.binding().runtime.generation, 1);
    assert_eq!(observation.session_binding_digest(), observation.binding().digest);
    assert_eq!(observation.observation().sender, "FixtureSeller");
    assert!(observation.frozen_manifest().is_some());
    assert_ne!(observation.remote_observation_id(), Uuid::nil());
}

#[tokio::test]
async fn allowlist_and_transient_failures_stay_pending() {
    let server = MailDouble::start().await.expect("loopback");
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (denied, _) =
        gateway(&server, MoogleMailWorld::Main, &["OtherSender"], Arc::clone(&profile));
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    assert!(matches!(
        read_envelope(&denied, &row).await,
        ConsignmentReadOutcome::Pending(ConsignmentReadPending::AllowlistDenied)
    ));

    for (fault, expected) in [
        (Fault::DetailAccountBusy, ConsignmentReadPending::AccountBusy),
        (Fault::DetailRateLimited, ConsignmentReadPending::RateLimited),
        (Fault::CookieInvalid, ConsignmentReadPending::TransientReadUnavailable),
    ] {
        let server = MailDouble::start().await.expect("loopback");
        server.set_fault(Some(fault));
        let (gateway, _) = gateway(
            &server,
            MoogleMailWorld::Main,
            &["FixtureSeller"],
            Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME)),
        );
        assert!(matches!(
            read_envelope(&gateway, &row).await,
            ConsignmentReadOutcome::Pending(reason) if reason == expected
        ));
    }
}

#[tokio::test]
async fn consignment_read_stages_pending_before_detail_get() {
    let server = Arc::new(MailDouble::start().await.expect("loopback"));
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, _) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let journal = RecordingJournal {
        server: Arc::clone(&server),
        stage_fails: false,
        pending: Vec::new(),
        events: Mutex::new(Vec::new()),
    };
    let outcome = gateway
        .read_consignment_envelope(
            &row,
            Instant::now() + Duration::from_secs(3),
            &journal,
            &poll_claim(),
        )
        .await;
    assert!(matches!(outcome, ConsignmentReadOutcome::Observed(_)));
    let events = journal.events.lock().expect("journal events");
    assert!(
        events.iter().any(|event| matches!(event, JournalEvent::Pending { requests_before: 0 }))
    );
    assert!(
        events.iter().any(|event| matches!(event, JournalEvent::Stage { requests_before_get: 0 }))
    );
    assert!(server.requests().iter().any(|request| request.contains("mid=900001")));
}

#[tokio::test]
async fn consignment_read_registration_failure_has_zero_gets() {
    let server = Arc::new(MailDouble::start().await.expect("loopback"));
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, _) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let journal = RecordingJournal {
        server: Arc::clone(&server),
        stage_fails: true,
        pending: Vec::new(),
        events: Mutex::new(Vec::new()),
    };
    let outcome = gateway
        .read_consignment_envelope(
            &row,
            Instant::now() + Duration::from_secs(3),
            &journal,
            &poll_claim(),
        )
        .await;
    assert!(matches!(
        outcome,
        ConsignmentReadOutcome::Pending(ConsignmentReadPending::TransientReadUnavailable)
    ));
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn consignment_read_keeps_earlier_pending_mid_before_any_get() {
    let server = Arc::new(MailDouble::start().await.expect("loopback"));
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, _) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let journal = RecordingJournal {
        server: Arc::clone(&server),
        stage_fails: false,
        pending: vec![MailPollReadSummary {
            external_mail_id: "899999".to_owned(),
            world_id: MoogleMailWorld::Main.as_str().to_owned(),
            sender: "FixtureSeller".to_owned(),
            subject: "consign".to_owned(),
            received_at: Timestamp::now(),
            list_world_marker: None,
            received_minute_raw: None,
        }],
        events: Mutex::new(Vec::new()),
    };
    let outcome = gateway
        .read_consignment_envelope(
            &row,
            Instant::now() + Duration::from_secs(3),
            &journal,
            &poll_claim(),
        )
        .await;
    assert!(
        matches!(
            outcome,
            ConsignmentReadOutcome::Pending(ConsignmentReadPending::TransientReadUnavailable)
        ),
        "实际结果 {outcome:?}"
    );
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn authenticated_but_unclear_manifest_is_observed_not_empty() {
    let server = MailDouble::start().await.expect("loopback");
    server.set_fault(Some(Fault::DetailProtocolDrift));
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, _) =
        gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], Arc::clone(&profile));
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let observation = observe(&gateway, &row).await;
    assert!(observation.observation().unknown_attachment);
    assert!(observation.observation().asset_count_unknown);
    assert!(observation.observation().items.is_empty());
    assert!(observation.frozen_manifest().is_none());
}

#[tokio::test]
async fn missing_authenticated_uid_stays_pending() {
    let server = MailDouble::start().await.expect("loopback");
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    profile.set("9999999", BOT_NAME);
    let (gateway, _) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let outcome = read_envelope(&gateway, &row).await;
    assert!(
        matches!(
            outcome,
            ConsignmentReadOutcome::Pending(ConsignmentReadPending::AuthenticatedUidUnavailable)
        ),
        "实际结果 {outcome:?}"
    );
}

#[tokio::test]
async fn renamed_uid_is_verified_but_same_name_new_uid_is_quarantined() {
    let server = MailDouble::start().await.expect("loopback");
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, _) =
        gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], Arc::clone(&profile));
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let observation = observe(&gateway, &row).await;
    let mut renamed = observation.binding().clone();
    renamed.player_name = "OldBot".to_owned();
    renamed.digest = renamed.compute_digest();
    assert_eq!(
        observation.mailbox_identity(Some(&renamed)),
        auction_domain::consignment::SenderIdentity::Verified
    );
    let mut changed_uid = observation.binding().clone();
    changed_uid.authenticated_uid = "1000001".to_owned();
    changed_uid.digest = changed_uid.compute_digest();
    assert_eq!(
        observation.mailbox_identity(Some(&changed_uid)),
        auction_domain::consignment::SenderIdentity::SameNameDifferentUid
    );
    let renamed_projection =
        observation.to_envelope_observation(Uuid::new_v4(), Some(&renamed), None);
    assert_eq!(
        renamed_projection.sender_identity,
        auction_domain::consignment::SenderIdentity::Verified
    );
    let changed_uid_projection =
        observation.to_envelope_observation(Uuid::new_v4(), Some(&changed_uid), None);
    assert_eq!(
        changed_uid_projection.sender_identity,
        auction_domain::consignment::SenderIdentity::SameNameDifferentUid
    );
}

#[tokio::test]
async fn wrong_recipient_and_world_are_observed_with_unproven_flags() {
    let server = MailDouble::start().await.expect("loopback");
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, _) =
        gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], Arc::clone(&profile));
    let recipient_row = auction_integrations::mooglemail::ConsignmentListRow {
        external_mail_id: "900010".to_owned(),
        ..list_row(LIST_MAIN, MoogleMailWorld::Main, "900001")
    };
    let recipient = observe(&gateway, &recipient_row).await;
    assert_eq!(recipient.observation().recipient, "OtherBot");
    assert_eq!(
        recipient.mailbox_identity(None),
        auction_domain::consignment::SenderIdentity::Unverified
    );
    let world_row = auction_integrations::mooglemail::ConsignmentListRow {
        external_mail_id: "900011".to_owned(),
        ..list_row(LIST_MAIN, MoogleMailWorld::Main, "900001")
    };
    let world = observe(&gateway, &world_row).await;
    assert_eq!(world.observation().world_marker, Some(MoogleMailWorld::Isekai));
    assert!(!world.observation().list_detail_consistent);
}

#[tokio::test]
async fn wrong_recipient_disposition_is_refused_without_post() {
    let server = MailDouble::start().await.expect("loopback");
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, _) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let row = auction_integrations::mooglemail::ConsignmentListRow {
        external_mail_id: "900010".to_owned(),
        ..list_row(LIST_MAIN, MoogleMailWorld::Main, "900001")
    };
    let observation = observe(&gateway, &row).await;
    let outcome = gateway
        .prepare_consignment_disposition(
            DispositionRequest::Take(target_for(&observation, 17)),
            Instant::now() + Duration::from_secs(3),
        )
        .await;
    assert!(matches!(outcome, Err(NotSentReason::IdentityChanged)));
    assert_eq!(server.write_requests(), 0);
}

#[tokio::test]
async fn duplicate_alias_and_same_minute_reads_are_distinct_observations() {
    let server = MailDouble::start().await.expect("loopback");
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, _) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let base = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let duplicate = observe(
        &gateway,
        &auction_integrations::mooglemail::ConsignmentListRow {
            external_mail_id: "900004".to_owned(),
            ..base.clone()
        },
    )
    .await;
    assert!(duplicate.observation().duplicate_raw_eid);
    let alias = observe(
        &gateway,
        &auction_integrations::mooglemail::ConsignmentListRow {
            external_mail_id: "900005".to_owned(),
            ..base.clone()
        },
    )
    .await;
    assert!(alias.observation().canonical_alias_ambiguous);
    let first = observe(&gateway, &base).await;
    let second = observe(
        &gateway,
        &auction_integrations::mooglemail::ConsignmentListRow {
            external_mail_id: "900002".to_owned(),
            ..base
        },
    )
    .await;
    assert_eq!(first.observation().received_minute_raw, second.observation().received_minute_raw);
    assert_ne!(first.remote_observation_id(), second.remote_observation_id());
}

#[tokio::test]
async fn take_full_envelope_proves_and_posts_once() {
    let server = MailDouble::start().await.expect("loopback");
    server.script_take_mail("900001", DETAIL_SINGLE.to_owned(), DETAIL_TAKE_AFTER.to_owned());
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, transport) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let observation = observe(&gateway, &row).await;
    let target = target_for(&observation, 7);
    let prepared = gateway
        .prepare_consignment_disposition(
            DispositionRequest::Take(target),
            Instant::now() + Duration::from_secs(3),
        )
        .await
        .expect("处置准备");
    let permit = permit_for(&prepared).await;
    let outcome = transport.execute_consignment_disposition(prepared, permit).await;
    let DispositionOutcome::Proven(proof) = outcome else {
        panic!("expected proven take, got {outcome:?}");
    };
    assert_eq!(proof.action(), DispositionAction::Take);
    assert_eq!(
        proof.validator_status(),
        auction_domain::consignment::ProofValidatorStatus::Unsigned
    );
    assert_eq!(proof.validator_version(), CONSIGNMENT_DISPOSITION_VALIDATOR_VERSION);
    let take = proof.take_observation().expect("Take 证明");
    // 替身在 POST 前的详情读取回放 DETAIL_SINGLE；独立解析该写前重读页面。
    let before_row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let expected_before_digest =
        parse_consignment_detail(DETAIL_SINGLE.as_bytes(), MoogleMailWorld::Main, &before_row)
            .manifest_digest();
    assert_eq!(take.observed_manifest_digest, expected_before_digest);
    assert_eq!(take.observed_manifest_digest, take.expected_manifest_digest);
    let after_row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    // 写后页面独立解析；它必须与冻结清单不同，但仍是该次 Take 的证明输入。
    let expected_after_digest =
        parse_consignment_detail(DETAIL_TAKE_AFTER.as_bytes(), MoogleMailWorld::Main, &after_row)
            .manifest_digest();
    assert_eq!(proof.after_page_digest(), Some(expected_after_digest));
    assert_ne!(expected_after_digest, take.expected_manifest_digest);
    assert!(proof.into_signed().is_none());
    assert_eq!(server.take_counts(), (1, 1));
    assert_eq!(server.write_requests(), 1);
}

#[tokio::test]
async fn take_proof_digest_binds_after_page_digest() {
    let first_after = DETAIL_TAKE_AFTER.to_owned();
    let second_after = DETAIL_TAKE_AFTER.replace(
        "Removed attachment: Fixture Blade",
        "Removed attachment: Fixture Blade\nfixture receipt note",
    );
    let (first_server, first_outcome) = execute_take("900001", DETAIL_SINGLE, first_after).await;
    let (second_server, second_outcome) = execute_take("900001", DETAIL_SINGLE, second_after).await;
    let DispositionOutcome::Proven(first) = first_outcome else {
        panic!("expected first proven take, got {first_outcome:?}");
    };
    let DispositionOutcome::Proven(second) = second_outcome else {
        panic!("expected second proven take, got {second_outcome:?}");
    };
    assert_ne!(first.after_page_digest(), second.after_page_digest());
    assert_ne!(first.proof_digest(), second.proof_digest());
    assert_eq!(first_server.take_counts(), (1, 1));
    assert_eq!(second_server.take_counts(), (1, 1));
}

#[tokio::test]
async fn take_unknown_write_response_is_sent_unproven() {
    let server = MailDouble::start().await.expect("loopback");
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, transport) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let observation = observe(&gateway, &row).await;
    let prepared = gateway
        .prepare_consignment_disposition(
            DispositionRequest::Take(target_for(&observation, 4)),
            Instant::now() + Duration::from_secs(3),
        )
        .await
        .expect("处置准备");
    server.set_fault(Some(Fault::WriteResultUnknown));
    let permit = permit_for(&prepared).await;
    assert!(matches!(
        transport.execute_consignment_disposition(prepared, permit).await,
        DispositionOutcome::SentUnproven { reason: SentUnprovenReason::ProofUnproven, .. }
    ));
    assert_eq!(server.write_requests(), 1);
}

#[tokio::test]
async fn take_missing_extra_or_duplicate_markers_stay_unproven() {
    let variants = [
        DETAIL_TAKE_AFTER.replace("Removed attachment: Fixture Blade", ""),
        DETAIL_TAKE_AFTER.replace("</textarea>", "\nRemoved attachment: Fixture Shield</textarea>"),
        DETAIL_TAKE_AFTER.replace(
            "Removed attachment: Fixture Blade",
            "Removed attachment: Fixture Blade\nRemoved attachment: Fixture Blade",
        ),
    ];
    for after in variants {
        let server = MailDouble::start().await.expect("loopback");
        server.script_take_mail("900001", DETAIL_SINGLE.to_owned(), after);
        let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
        let (gateway, transport) =
            gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
        let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
        let observation = observe(&gateway, &row).await;
        let prepared = gateway
            .prepare_consignment_disposition(
                DispositionRequest::Take(target_for(&observation, 3)),
                Instant::now() + Duration::from_secs(3),
            )
            .await
            .expect("处置准备");
        let permit = permit_for(&prepared).await;
        assert!(matches!(
            transport.execute_consignment_disposition(prepared, permit).await,
            DispositionOutcome::SentUnproven { reason: SentUnprovenReason::ProofUnproven, .. }
        ));
        assert_eq!(server.take_counts(), (1, 1));
    }
}

#[tokio::test]
async fn take_after_remaining_attachment_stays_unproven() {
    let (server, outcome) =
        execute_take("900001", DETAIL_SINGLE, take_after_with_attachment()).await;
    assert!(matches!(
        outcome,
        DispositionOutcome::SentUnproven { reason: SentUnprovenReason::ProofUnproven, .. }
    ));
    assert_eq!(server.take_counts(), (1, 1));
    assert_eq!(server.write_requests(), 1);
}

#[tokio::test]
async fn take_after_body_or_identity_change_stays_unproven() {
    let reservation = "C1-".to_owned() + &"A".repeat(43);
    let changed_body =
        DETAIL_TAKE_AFTER.replace(&reservation, &("C1-".to_owned() + &"B".repeat(43)));
    let changed_identity =
        DETAIL_TAKE_AFTER.replace("value=\"FixtureSeller\"", "value=\"OtherSeller\"");
    for after in [changed_body, changed_identity] {
        let (server, outcome) = execute_take("900001", DETAIL_SINGLE, after).await;
        assert!(matches!(
            outcome,
            DispositionOutcome::SentUnproven { reason: SentUnprovenReason::ProofUnproven, .. }
        ));
        assert_eq!(server.take_counts(), (1, 1));
        assert_eq!(server.write_requests(), 1);
    }
}

#[tokio::test]
async fn take_cash_receipt_direction_mismatch_stays_unproven() {
    let after = DETAIL_TAKE_AFTER.replace(
        "Removed attachment: Fixture Blade",
        "Removed attachment: Fixture Blade\nCoD Paid: 10 Credits",
    );
    let (server, outcome) = execute_take("900001", DETAIL_SINGLE, after).await;
    assert!(matches!(
        outcome,
        DispositionOutcome::SentUnproven { reason: SentUnprovenReason::ProofUnproven, .. }
    ));
    assert_eq!(server.take_counts(), (1, 1));
    assert_eq!(server.write_requests(), 1);
}

#[tokio::test]
async fn take_cod_receipt_amount_mismatch_stays_unproven() {
    let after = DETAIL_TAKE_AFTER.replace(
        "Removed attachment: Fixture Blade",
        "Removed attachment: Fixture Blade\nCoD Paid: 999 Credits",
    );
    let (server, outcome) = execute_take("900009", DETAIL_COD, after).await;
    assert!(matches!(
        outcome,
        DispositionOutcome::SentUnproven { reason: SentUnprovenReason::ProofUnproven, .. }
    ));
    assert_eq!(server.take_counts(), (1, 1));
    assert_eq!(server.write_requests(), 1);
}

#[tokio::test]
async fn take_unparseable_after_page_stays_unproven() {
    let (server, outcome) = execute_take("900001", DETAIL_SINGLE, DETAIL_MISSING.to_owned()).await;
    assert!(matches!(
        outcome,
        DispositionOutcome::SentUnproven { reason: SentUnprovenReason::ProofUnproven, .. }
    ));
    assert_eq!(server.take_counts(), (1, 1));
    assert_eq!(server.write_requests(), 1);
}

#[tokio::test]
async fn take_same_name_without_unique_eid_stays_unproven() {
    let server = MailDouble::start().await.expect("loopback");
    server.script_take_mail(
        "900012",
        DETAIL_SAME_NAME.to_owned(),
        DETAIL_TAKE_AFTER_SAME_NAME.to_owned(),
    );
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, transport) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let row = auction_integrations::mooglemail::ConsignmentListRow {
        external_mail_id: "900012".to_owned(),
        ..list_row(LIST_MAIN, MoogleMailWorld::Main, "900001")
    };
    let observation = observe(&gateway, &row).await;
    let prepared = gateway
        .prepare_consignment_disposition(
            DispositionRequest::Take(target_for(&observation, 5)),
            Instant::now() + Duration::from_secs(3),
        )
        .await
        .expect("处置准备");
    let permit = permit_for(&prepared).await;
    assert!(matches!(
        transport.execute_consignment_disposition(prepared, permit).await,
        DispositionOutcome::SentUnproven { reason: SentUnprovenReason::ProofUnproven, .. }
    ));
    assert_eq!(server.take_counts(), (1, 1));
}

#[tokio::test]
async fn take_manifest_change_before_post_has_zero_posts() {
    let server = MailDouble::start().await.expect("loopback");
    server.script_take_mail("900001", DETAIL_SINGLE.to_owned(), DETAIL_TAKE_AFTER.to_owned());
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, transport) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let observation = observe(&gateway, &row).await;
    let prepared = gateway
        .prepare_consignment_disposition(
            DispositionRequest::Take(target_for(&observation, 9)),
            Instant::now() + Duration::from_secs(3),
        )
        .await
        .expect("处置准备");
    server.script_take_mail(
        "900001",
        DETAIL_SINGLE.replace("FixtureSeller", "OtherSeller"),
        DETAIL_TAKE_AFTER.to_owned(),
    );
    let permit = permit_for(&prepared).await;
    assert!(matches!(
        transport.execute_consignment_disposition(prepared, permit).await,
        DispositionOutcome::NotSent { reason: NotSentReason::ManifestChanged }
    ));
    assert_eq!(server.take_counts(), (0, 0));
    assert_eq!(server.write_requests(), 0);
}

#[tokio::test]
async fn return_receipt_proves_and_posts_once() {
    let server = MailDouble::start().await.expect("loopback");
    server.script_return_mail("900001", DETAIL_SINGLE.to_owned(), DETAIL_RETURN_RECEIPT.to_owned());
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, transport) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let observation = observe(&gateway, &row).await;
    let target = target_for(&observation, 11);
    let expected = ExpectedReturn::new(observation.frozen_manifest().expect("冻结清单"));
    let prepared = gateway
        .prepare_consignment_disposition(
            DispositionRequest::Return { target: Box::new(target), expected_return: expected },
            Instant::now() + Duration::from_secs(3),
        )
        .await
        .expect("处置准备");
    let permit = permit_for(&prepared).await;
    let outcome = transport.execute_consignment_disposition(prepared, permit).await;
    let DispositionOutcome::Proven(proof) = outcome else {
        panic!("expected proven return, got {outcome:?}");
    };
    assert_eq!(proof.action(), DispositionAction::Return);
    assert!(proof.take_observation().is_none());
    assert!(proof.into_signed().is_none());
    assert_eq!(server.return_counts(), (1, 1));
    assert_eq!(server.write_requests(), 1);
}

#[tokio::test]
async fn return_manifest_change_before_post_has_zero_posts() {
    let server = MailDouble::start().await.expect("loopback");
    server.script_return_mail("900001", DETAIL_SINGLE.to_owned(), DETAIL_RETURN_RECEIPT.to_owned());
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, transport) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let observation = observe(&gateway, &row).await;
    let target = target_for(&observation, 13);
    let expected = ExpectedReturn::new(observation.frozen_manifest().expect("冻结清单"));
    let prepared = gateway
        .prepare_consignment_disposition(
            DispositionRequest::Return { target: Box::new(target), expected_return: expected },
            Instant::now() + Duration::from_secs(3),
        )
        .await
        .expect("处置准备");
    server.script_return_mail(
        "900001",
        DETAIL_SINGLE.replace("consign", "consign-changed"),
        DETAIL_RETURN_RECEIPT.to_owned(),
    );
    let permit = permit_for(&prepared).await;
    assert!(matches!(
        transport.execute_consignment_disposition(prepared, permit).await,
        DispositionOutcome::NotSent { reason: NotSentReason::ManifestChanged }
    ));
    assert_eq!(server.return_counts(), (0, 0));
    assert_eq!(server.write_requests(), 0);
}

#[tokio::test]
async fn return_without_or_with_mismatched_system_receipt_stays_unproven() {
    let asset_mismatch = DETAIL_RETURN_RECEIPT.replace("900001", "900002");
    let sender_mismatch =
        DETAIL_RETURN_RECEIPT.replace("value=\"MoogleMail\"", "value=\"OtherSender\"");
    for after in [DETAIL_SINGLE.to_owned(), asset_mismatch, sender_mismatch] {
        let (server, outcome) = execute_return("900001", DETAIL_SINGLE, after).await;
        assert!(matches!(
            outcome,
            DispositionOutcome::SentUnproven { reason: SentUnprovenReason::ProofUnproven, .. }
        ));
        assert_eq!(server.return_counts(), (1, 1));
        assert_eq!(server.write_requests(), 1);
    }
}

#[tokio::test]
async fn identity_change_between_prepare_and_execute_has_zero_posts() {
    let server = MailDouble::start().await.expect("loopback");
    server.script_take_mail("900001", DETAIL_SINGLE.to_owned(), DETAIL_TAKE_AFTER.to_owned());
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, transport) =
        gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], Arc::clone(&profile));
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let observation = observe(&gateway, &row).await;
    let prepared = gateway
        .prepare_consignment_disposition(
            DispositionRequest::Take(target_for(&observation, 15)),
            Instant::now() + Duration::from_secs(3),
        )
        .await
        .expect("处置准备");
    profile.set(BOT_UID, "OtherBot");
    let permit = permit_for(&prepared).await;
    assert!(matches!(
        transport.execute_consignment_disposition(prepared, permit).await,
        DispositionOutcome::NotSent { reason: NotSentReason::IdentityChanged }
    ));
    assert_eq!(server.take_counts(), (0, 0));
    assert_eq!(server.write_requests(), 0);
}

#[tokio::test]
async fn permit_from_another_work_is_rejected_without_post() {
    let server = MailDouble::start().await.expect("loopback");
    server.script_take_mail("900001", DETAIL_SINGLE.to_owned(), DETAIL_TAKE_AFTER.to_owned());
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, transport) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let observation = observe(&gateway, &row).await;
    let first = gateway
        .prepare_consignment_disposition(
            DispositionRequest::Take(target_for(&observation, 17)),
            Instant::now() + Duration::from_secs(3),
        )
        .await
        .expect("first preparation");
    let permit = permit_with(
        Uuid::new_v4(),
        Uuid::new_v4(),
        99,
        first.session_binding_digest(),
        first.request().target().frozen.manifest_digest,
    )
    .await;
    assert!(matches!(
        transport.execute_consignment_disposition(first, permit).await,
        DispositionOutcome::NotSent { reason: NotSentReason::PermitMismatch }
    ));
    assert_eq!(server.take_counts(), (0, 0));
    assert_eq!(server.write_requests(), 0);
}

#[tokio::test]
async fn permit_with_only_another_work_id_is_rejected_without_post() {
    let server = MailDouble::start().await.expect("loopback");
    server.script_take_mail("900001", DETAIL_SINGLE.to_owned(), DETAIL_TAKE_AFTER.to_owned());
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (gateway, transport) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let observation = observe(&gateway, &row).await;
    let prepared = gateway
        .prepare_consignment_disposition(
            DispositionRequest::Take(target_for(&observation, 19)),
            Instant::now() + Duration::from_secs(3),
        )
        .await
        .expect("处置准备");
    let target = prepared.request().target();
    let permit = permit_with(
        Uuid::new_v4(),
        target.delivery_id,
        target.fence,
        prepared.session_binding_digest(),
        target.frozen.manifest_digest,
    )
    .await;
    assert!(matches!(
        transport.execute_consignment_disposition(prepared, permit).await,
        DispositionOutcome::NotSent { reason: NotSentReason::PermitMismatch }
    ));
    assert_eq!(server.take_counts(), (0, 0));
    assert_eq!(server.write_requests(), 0);
}

const LIST_MAIN: &str = include_str!("fixtures/mooglemail/consign-list-main.html");
const LIST_ISEKAI: &str = include_str!("fixtures/mooglemail/consign-list-isekai.html");
const DETAIL_SINGLE: &str = include_str!("fixtures/mooglemail/consign-detail-main-single.html");
const DETAIL_ISEKAI_MULTI: &str =
    include_str!("fixtures/mooglemail/consign-detail-isekai-multi.html");
const DETAIL_DUPLICATE_EID: &str =
    include_str!("fixtures/mooglemail/consign-detail-main-duplicate-eid.html");
const DETAIL_ALIAS: &str = include_str!("fixtures/mooglemail/consign-detail-main-alias.html");
const DETAIL_UNKNOWN: &str = include_str!("fixtures/mooglemail/consign-detail-main-unknown.html");
const DETAIL_BAD_COUNT: &str =
    include_str!("fixtures/mooglemail/consign-detail-main-bad-count.html");
const DETAIL_CREDITS: &str = include_str!("fixtures/mooglemail/consign-detail-main-credits.html");
const DETAIL_COD: &str = include_str!("fixtures/mooglemail/consign-detail-main-cod.html");
const DETAIL_WRONG_RECIPIENT: &str =
    include_str!("fixtures/mooglemail/consign-detail-main-wrong-recipient.html");
const DETAIL_WRONG_WORLD: &str =
    include_str!("fixtures/mooglemail/consign-detail-main-wrong-world.html");
const DETAIL_TAKE_AFTER: &str =
    include_str!("fixtures/mooglemail/consign-take-after-main-single.html");
const DETAIL_SAME_NAME: &str =
    include_str!("fixtures/mooglemail/consign-detail-main-same-name.html");
const DETAIL_TAKE_AFTER_SAME_NAME: &str =
    include_str!("fixtures/mooglemail/consign-take-after-main-same-name.html");
const DETAIL_RETURN_RECEIPT: &str =
    include_str!("fixtures/mooglemail/consign-return-receipt-main.html");
const DETAIL_MISSING: &str = include_str!("fixtures/mooglemail/inboxdetail-missing.html");
const READ_MAIN: &str = include_str!("fixtures/mooglemail/read-main.html");
const READ_RTS: &str = include_str!("fixtures/mooglemail/readdetail-read-3125357.html");

const COOKIE: &str = "ipb_member_id=fixture; ipb_pass_hash=fixture";
const BOT_NAME: &str = "FixtureBot";
const BOT_UID: &str = "1000000";

fn list_row(
    document: &str,
    world: MoogleMailWorld,
    mid: &str,
) -> auction_integrations::mooglemail::ConsignmentListRow {
    parse_consignment_list_row(document.as_bytes(), world, mid)
        .expect("列表结构有效")
        .expect("目标 MID 存在")
}

#[test]
fn main_single_observation_preserves_order_identity_and_marker_inputs() {
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    assert_eq!(row.received_minute_raw, "2026-11-02 03:45");
    assert_eq!(row.sender, "FixtureSeller");
    assert_eq!(row.subject, "consign");
    let observation =
        parse_consignment_detail(DETAIL_SINGLE.as_bytes(), MoogleMailWorld::Main, &row);
    assert_eq!(observation.parser_version, CONSIGNMENT_PARSER_VERSION);
    assert_eq!(observation.recipient, BOT_NAME);
    assert_eq!(observation.sender, "FixtureSeller");
    assert_eq!(observation.subject, "consign");
    assert!(observation.subject_is_consign());
    assert_eq!(observation.received_minute_raw, "2026-11-02 03:45");
    assert_eq!(observation.credits, Some(0));
    assert_eq!(observation.cod, Some(0));
    assert!(observation.returnable);
    assert!(observation.claimable);
    assert!(observation.actions_proven);
    assert!(observation.list_detail_consistent);
    assert!(!observation.duplicate_raw_eid);
    assert!(!observation.canonical_alias_ambiguous);
    assert!(!observation.unknown_attachment);
    assert!(!observation.asset_count_unknown);
    assert_eq!(observation.items.len(), 1);
    let item = &observation.items[0];
    assert_eq!(item.ordinal, 1);
    assert_eq!(item.kind, EnvelopeItemKind::Equipment);
    assert_eq!(item.raw_eid.as_deref(), Some("900001"));
    assert_eq!(item.canonical_eid.as_deref(), Some("900001"));
    assert_eq!(item.name, "Fixture Blade");
    assert!(item.key.is_some());
    let frozen = observation.frozen_manifest().expect("单件清单可冻结");
    assert_eq!(frozen.attachments.len(), 1);
    assert_eq!(frozen.manifest_digest, observation.manifest_digest());
    let envelope = observation.to_envelope_observation(
        Uuid::new_v4(),
        [7; 32],
        auction_domain::consignment::SenderIdentity::Verified,
        false,
        None,
    );
    assert_eq!(envelope.asset_count, Some(1));
    assert_eq!(envelope.items[0].world, Some(SettlementWorld::Main));
}

#[test]
fn isekai_multi_observation_keeps_original_order() {
    let row = list_row(LIST_ISEKAI, MoogleMailWorld::Isekai, "910001");
    assert_eq!(row.world_marker, Some(MoogleMailWorld::Isekai));
    let observation =
        parse_consignment_detail(DETAIL_ISEKAI_MULTI.as_bytes(), MoogleMailWorld::Isekai, &row);
    assert_eq!(observation.world_marker, Some(MoogleMailWorld::Isekai));
    assert!(observation.list_detail_consistent);
    assert_eq!(observation.items.len(), 2);
    assert_eq!(observation.items[0].raw_eid.as_deref(), Some("910001"));
    assert_eq!(observation.items[1].raw_eid.as_deref(), Some("910002"));
    assert_eq!(observation.items[0].name, "Fixture Blade");
    assert_eq!(observation.items[1].name, "Fixture Shield");
    assert!(observation.frozen_manifest().is_some());
}

#[test]
fn reservation_codes_remain_in_body_for_domain_ownership_matching() {
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let observation =
        parse_consignment_detail(DETAIL_SINGLE.as_bytes(), MoogleMailWorld::Main, &row);
    let tokens = auction_domain::consignment::parse_deposit_body(
        observation.body.as_bytes(),
        auction_domain::consignment::DEPOSIT_ITEM_LIMIT,
        auction_domain::consignment::DEPOSIT_BYTE_LIMIT,
    )
    .expect("寄售正文可解析");
    assert_eq!(tokens.len(), 1);
    let code = auction_domain::consignment::format_token(*tokens[0].as_bytes());
    assert_eq!(code.as_str(), "C1-".to_owned() + &"A".repeat(43));
    // 预约码归属、卖家与末件失败由 4A 领域决定；协议层只保证原文与摘要未被丢弃。
}

#[test]
fn observation_debug_display_and_diagnostics_hide_reservation_and_equipment_secrets() {
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let observation =
        parse_consignment_detail(DETAIL_SINGLE.as_bytes(), MoogleMailWorld::Main, &row);
    let code = "C1-".to_owned() + &"A".repeat(43);
    let equipment_key = "0123456789";
    let debug = format!("{observation:?}");
    assert!(!debug.contains(&code));
    assert!(!debug.contains(equipment_key));
    assert!(!format!("{:?}", observation.items).contains(equipment_key));

    let display = MailDisplaySnapshot::redact_stored_body(&observation.body);
    assert!(!display.contains(&code));
    let diagnostic = MailDisplaySnapshot::redact_failure_diagnostic(&format!(
        "寄售失败 {code} https://hentaiverse.org/equip/900001/{equipment_key}"
    ));
    assert!(!diagnostic.contains(&code));
    assert!(!diagnostic.contains(equipment_key));
}

#[test]
fn duplicate_raw_eid_and_canonical_alias_are_controlled_observations() {
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let duplicate = parse_consignment_detail(
        DETAIL_DUPLICATE_EID.as_bytes(),
        MoogleMailWorld::Main,
        &auction_integrations::mooglemail::ConsignmentListRow {
            external_mail_id: "900004".to_owned(),
            ..row.clone()
        },
    );
    assert!(duplicate.duplicate_raw_eid);
    assert_eq!(duplicate.items.len(), 2);
    assert!(duplicate.frozen_manifest().is_none());

    let alias = parse_consignment_detail(
        DETAIL_ALIAS.as_bytes(),
        MoogleMailWorld::Main,
        &auction_integrations::mooglemail::ConsignmentListRow {
            external_mail_id: "900005".to_owned(),
            ..row
        },
    );
    assert!(alias.canonical_alias_ambiguous);
    assert_eq!(alias.items[0].raw_eid.as_deref(), Some("00042"));
    assert_eq!(alias.items[1].raw_eid.as_deref(), Some("42"));
    assert_eq!(alias.items[0].canonical_eid.as_deref(), Some("42"));
    assert_eq!(alias.items[1].canonical_eid.as_deref(), Some("42"));
    assert!(alias.frozen_manifest().is_none());
}

#[test]
fn unknown_attachment_count_and_amounts_are_preserved() {
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let unknown = parse_consignment_detail(
        DETAIL_UNKNOWN.as_bytes(),
        MoogleMailWorld::Main,
        &auction_integrations::mooglemail::ConsignmentListRow {
            external_mail_id: "900006".to_owned(),
            ..row.clone()
        },
    );
    assert!(unknown.unknown_attachment);
    assert_eq!(unknown.items[0].kind, EnvelopeItemKind::Unknown);
    assert!(unknown.frozen_manifest().is_none());

    let bad_count = parse_consignment_detail(
        DETAIL_BAD_COUNT.as_bytes(),
        MoogleMailWorld::Main,
        &auction_integrations::mooglemail::ConsignmentListRow {
            external_mail_id: "900007".to_owned(),
            ..row.clone()
        },
    );
    assert_eq!(bad_count.reported_attachment_count, Some(2));
    assert!(bad_count.asset_count_unknown);

    let credits = parse_consignment_detail(
        DETAIL_CREDITS.as_bytes(),
        MoogleMailWorld::Main,
        &auction_integrations::mooglemail::ConsignmentListRow {
            external_mail_id: "900008".to_owned(),
            ..row.clone()
        },
    );
    assert_eq!(credits.credits, Some(49_000));
    assert_eq!(credits.items[0].kind, EnvelopeItemKind::Credits);
    assert_eq!(credits.items[0].amount, Some(49_000));
    assert!(credits.frozen_manifest().is_none());

    let cod = parse_consignment_detail(
        DETAIL_COD.as_bytes(),
        MoogleMailWorld::Main,
        &auction_integrations::mooglemail::ConsignmentListRow {
            external_mail_id: "900009".to_owned(),
            ..row
        },
    );
    assert_eq!(cod.cod, Some(1_234));
    assert_eq!(cod.items[0].kind, EnvelopeItemKind::Equipment);
}

#[test]
fn recipient_and_world_conflicts_are_visible_not_dropped() {
    let row = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let wrong_recipient = parse_consignment_detail(
        DETAIL_WRONG_RECIPIENT.as_bytes(),
        MoogleMailWorld::Main,
        &auction_integrations::mooglemail::ConsignmentListRow {
            external_mail_id: "900010".to_owned(),
            ..row.clone()
        },
    );
    assert_eq!(wrong_recipient.recipient, "OtherBot");
    let wrong_world = parse_consignment_detail(
        DETAIL_WRONG_WORLD.as_bytes(),
        MoogleMailWorld::Main,
        &auction_integrations::mooglemail::ConsignmentListRow {
            external_mail_id: "900011".to_owned(),
            ..row
        },
    );
    assert_eq!(wrong_world.world_marker, Some(MoogleMailWorld::Isekai));
    assert!(!wrong_world.list_detail_consistent);
}

#[test]
fn same_minute_messages_remain_distinct_by_mid() {
    let first = list_row(LIST_MAIN, MoogleMailWorld::Main, "900001");
    let second = list_row(LIST_MAIN, MoogleMailWorld::Main, "900002");
    assert_eq!(first.received_minute_raw, second.received_minute_raw);
    assert_eq!(first.received_at, second.received_at);
    assert_ne!(first.external_mail_id, second.external_mail_id);
}

#[test]
fn new_entry_does_not_change_auction_detail_parsing() {
    let mut seen = HashSet::new();
    let page = MoogleMailParser
        .scan_read_mail_page(READ_MAIN.as_bytes(), MoogleMailWorld::Main, 0, &mut seen)
        .expect("真实 Read Mail 列表");
    let summary = page
        .summaries
        .iter()
        .find(|summary| summary.external_mail_id == "3125357")
        .expect("含 RTS 邮件");
    let message =
        MoogleMailParser.parse_detail(READ_RTS.as_bytes(), summary).expect("拍卖详情解析不变");
    assert_eq!(message.sender, "MoogleMail");
    assert_eq!(message.subject, "[RTS] Auction payment overdue");
    assert!(message.attachments.is_empty());
    let row = list_row(READ_MAIN, MoogleMailWorld::Main, "3125357");
    let observation = parse_consignment_detail(READ_RTS.as_bytes(), MoogleMailWorld::Main, &row);
    assert!(!observation.subject_is_consign());
}

/// 原始 HTML 经网关观察，再补上应用层证明的卖家、可交易与历史，交给领域整封决定。
/// `take_ready` 由调用方给定：生产为 false，这里为 true 表示取得验证器签收后的路由。
fn decide_observed(
    observed: &auction_integrations::mooglemail::BoundConsignmentObservation,
    untradeable_ordinals: &[u32],
    take_ready: bool,
    sender_verified: bool,
) -> auction_domain::consignment::deposit::EnvelopeRoute {
    use auction_domain::UserId;
    use auction_domain::consignment::deposit::{
        EnvelopeCapabilities, LockedEnvelopeFacts, ReservationBinding, ReservationState,
        SenderIdentity, decide_envelope,
    };
    use auction_domain::consignment::{ListingEquipmentIdentity, ListingVersion, OriginalMailKey};

    let seller = UserId::from_uuid(Uuid::from_u128(7));
    let world = observed.binding().world;
    let mut observation = observed.to_envelope_observation(Uuid::new_v4(), None, None);
    if !sender_verified {
        observation.sender_identity = SenderIdentity::Unverified;
    }
    let world_code = match world {
        SettlementWorld::Main => "main",
        SettlementWorld::Isekai => "isekai",
    };
    let mut reservations = Vec::new();
    for item in &mut observation.items {
        item.seller_user_id = Some(seller);
        item.tradeable = !untradeable_ordinals.contains(&item.ordinal);
        item.history_ok = true;
        let Some(eid) = item.canonical_eid.clone() else {
            continue;
        };
        reservations.push(ReservationBinding {
            reservation_id: Uuid::from_u128(1_000 + u128::from(item.ordinal)),
            generation: 1,
            listing_id: Uuid::from_u128(2_000 + u128::from(item.ordinal)),
            listing_version: ListingVersion::new(3).expect("版本"),
            seller_user_id: seller,
            equipment: ListingEquipmentIdentity::new(world_code, &eid).expect("装备身份"),
            expires_at: Timestamp::UNIX_EPOCH + auction_time::Duration::hours(1),
            state: ReservationState::Active,
        });
    }
    decide_envelope(&LockedEnvelopeFacts {
        mail_key: Some(
            OriginalMailKey::new(
                &observed.binding().authenticated_uid,
                world,
                observed.observation().external_mail_id.as_str(),
            )
            .expect("原信键"),
        ),
        observation,
        reservations,
        envelope_seller_user_id: Some(seller),
        capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready },
        read_failure: false,
        already_decided: false,
        now: Timestamp::UNIX_EPOCH,
    })
}

#[tokio::test]
async fn isekai_and_main_raw_html_decide_accept_return_quarantine() {
    use auction_domain::consignment::deposit::{
        EnvelopeDecision, EnvelopeRoute, QuarantineReason, ReturnItemReason,
    };

    let server = MailDouble::start().await.expect("loopback");
    let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
    let (main_gateway, _) =
        gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], Arc::clone(&profile));
    let (isekai_gateway, _) =
        gateway(&server, MoogleMailWorld::Isekai, &["FixtureSeller"], profile);
    let main_single =
        observe(&main_gateway, &list_row(LIST_MAIN, MoogleMailWorld::Main, "900001")).await;
    let main_unknown = observe(
        &main_gateway,
        &auction_integrations::mooglemail::ConsignmentListRow {
            external_mail_id: "900006".to_owned(),
            ..list_row(LIST_MAIN, MoogleMailWorld::Main, "900001")
        },
    )
    .await;
    let isekai_multi =
        observe(&isekai_gateway, &list_row(LIST_ISEKAI, MoogleMailWorld::Isekai, "910001")).await;
    assert_eq!(main_single.binding().world, SettlementWorld::Main);
    assert_eq!(isekai_multi.binding().world, SettlementWorld::Isekai);

    // 主世界：整封可交易且签收后就绪 -> 接受；一件不可交易 -> 整封拒退；未知附件 -> 隔离。
    let EnvelopeRoute::Consignment(EnvelopeDecision::Accept(items)) =
        decide_observed(&main_single, &[], true, true)
    else {
        panic!("主世界应整封接受");
    };
    assert_eq!(items.len(), 1);
    let EnvelopeRoute::Consignment(EnvelopeDecision::Return(manifest)) =
        decide_observed(&main_single, &[1], true, true)
    else {
        panic!("主世界不可交易件应整封拒退");
    };
    assert_eq!(manifest.items[0].reason, ReturnItemReason::NotTradeable);
    assert_eq!(
        decide_observed(&main_unknown, &[], true, true),
        EnvelopeRoute::Consignment(EnvelopeDecision::Quarantine(
            QuarantineReason::UnknownAttachment
        ))
    );

    // 异世界多件：全部通过才接受，保持原序；末项失败整封拒退，不部分接受。
    let EnvelopeRoute::Consignment(EnvelopeDecision::Accept(items)) =
        decide_observed(&isekai_multi, &[], true, true)
    else {
        panic!("异世界应整封接受");
    };
    assert_eq!(items.iter().map(|item| item.ordinal).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(items[1].equipment.world_id(), "isekai");
    let EnvelopeRoute::Consignment(EnvelopeDecision::Return(manifest)) =
        decide_observed(&isekai_multi, &[2], true, true)
    else {
        panic!("末项失败时整封拒退，不得越过");
    };
    assert_eq!(manifest.items.len(), 2);
    assert_eq!(manifest.items[1].reason, ReturnItemReason::NotTradeable);
    assert_eq!(
        decide_observed(&isekai_multi, &[], true, false),
        EnvelopeRoute::Consignment(EnvelopeDecision::Quarantine(
            QuarantineReason::IdentityUnproven
        ))
    );

    // 生产的取得就绪为 false：两个世界的可接受整封都进隔离，不消费预约。
    for observed in [&main_single, &isekai_multi] {
        assert_eq!(
            decide_observed(observed, &[], false, true),
            EnvelopeRoute::Consignment(EnvelopeDecision::Quarantine(
                QuarantineReason::TakeUnproven
            ))
        );
    }

    // 原始页面证明不了可取得（页面状态不是未读的 1，或缺少 Take 入口）时，即使取得已签收也只能隔离：
    // takeable 由页面派生，不能因就绪为真而放宽（资产外写的安全边界）。
    for detail in [
        DETAIL_SINGLE.replace("var mail_state = 1;", "var mail_state = 2;"),
        DETAIL_SINGLE.replace(" onclick=\"mooglemail.remove_attachment(0)\"", ""),
    ] {
        let server = MailDouble::start().await.expect("loopback");
        server.script_take_mail("900001", detail, DETAIL_TAKE_AFTER.to_owned());
        let profile = Arc::new(FixtureProfile::new(BOT_UID, BOT_NAME));
        let (gateway, _) = gateway(&server, MoogleMailWorld::Main, &["FixtureSeller"], profile);
        let observed =
            observe(&gateway, &list_row(LIST_MAIN, MoogleMailWorld::Main, "900001")).await;
        // 观察层直接断言“不可证明可取得”，而不是“可证明不可取得”（Some(false)）。
        assert_eq!(observed.to_envelope_observation(Uuid::new_v4(), None, None).takeable, None);
        assert_eq!(
            decide_observed(&observed, &[], true, true),
            EnvelopeRoute::Consignment(EnvelopeDecision::Quarantine(
                QuarantineReason::TakeUnproven
            ))
        );
    }
}
