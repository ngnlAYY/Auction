//! Admin v2 隔离与运营止售处理器的离线路由测试。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::consignment::{
    AdminDepositAction, AdminDepositDetail, AdminDepositQuery, AdminListingDetailV2,
    AdminListingQuery, AdminListingRead, AdminListingReadV2, AdminListingRecordV2,
    AdminObservationItem, AdminQuarantineObservation, ApplyListingLookupCommand,
    ApplyMailProofCommand, AutoCloseListingCommand, ConsignmentError, ConsignmentFuture,
    CustodyControlStore, DepositBusinessRejection, DepositCommandService, DepositRead,
    DepositStatus, DispositionStore, EnvelopeStore, ExpireReservationCommand,
    IssueReservationCommand, ListingAutoCloseEffect, ListingBusinessRejection, ListingClosed,
    ListingCommandOutcome, ListingCommandStore, ListingLookupEffect, ListingPriceReceipt,
    ListingReadPage, ListingRegistered, ListingStateChange, ManageListingService,
    OperationsHoldDisposition, OperationsHoldReceipt, OwnedDepositInstructions, OwnedListingReadV2,
    PrivateListingQuery, PrivateListingV2, QuarantineState, RecheckQuarantineCommand,
    RecheckReceipt, RegisterListingCommand, ReleaseOperationsHoldCommand, ReleaseQuarantineCommand,
    ReleaseReceipt, ReservationReceipt, ReservationRetired, ReservationStore, SeasonCloseCommand,
    SeasonCloseReceipt, SeasonNoticeCommand, SeasonNoticeReceipt, SeasonStore,
    SetOperationsHoldCommand, StartDispositionWriteCommand, WithdrawDepositCommand,
    WithdrawDepositReceipt, WriteStartOutcome,
};
use auction_application::core::{RequestHash, SessionError};
use auction_application::public::{AdminLoginResult, AdminSessionIdentity, AdminSessionPort};
use auction_domain::consignment::deposit::EnvelopeItemKind;
use auction_domain::consignment::deposit::QuarantineReason;
use auction_domain::consignment::deposit::ReleaseKind;
use auction_domain::consignment::{
    HeldOffMarketReason, ListingError, ListingState, ListingVersion as DomainVersion,
};
use auction_domain::{CommandId, SettlementWorld, UserId};
use auction_time::Timestamp;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use axum::response::Response;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::{AdminDepositCapabilities, AdminDepositState, mount};
use crate::admin::ADMIN_CSRF_HEADER;
use crate::admin::consignment::AdminConsignmentState;
use crate::admin::consignment::mount as mount_v1_admin;
use crate::middleware::security::{AdminHttpSecurity, HttpSecurityOptions};
use crate::state::AdminApiState;

const DETAIL: &str = "/api/v2/admin/consignment/deposits";
const LISTINGS: &str = "/api/v2/admin/consignment/listings";
const QUARANTINE: Uuid = fixture_uuid(0x51);
const MISSING: Uuid = fixture_uuid(0x52);
const LISTING: Uuid = fixture_uuid(0x53);
const UNMANAGED: Uuid = fixture_uuid(0x54);
const SALE_PENDING: Uuid = fixture_uuid(0x55);
const WITHDRAWAL_PENDING: Uuid = fixture_uuid(0x56);
const CLOSED: Uuid = fixture_uuid(0x57);
const COMMAND: Uuid = fixture_uuid(0x60);
const OTHER_COMMAND: Uuid = fixture_uuid(0x61);
const HOLD: Uuid = fixture_uuid(0x70);
const ACTOR: &str = "fixture-admin";
const ORIGIN: &str = "https://admin.example.invalid";
const REASON: &str = "运营夹具：重复记录";

const fn fixture_uuid(suffix: u128) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 + suffix)
}

fn time() -> Timestamp {
    Timestamp::parse_from_rfc3339("2026-11-21T00:00:00Z").expect("夹具时刻")
}

fn observation(ordinal: u32) -> AdminQuarantineObservation {
    AdminQuarantineObservation {
        observation_id: fixture_uuid(0x80 + u128::from(ordinal)),
        observed_at: time(),
        parser_version: "p4d2-v1".to_owned(),
        digest: [u8::try_from(ordinal).expect("序号"); 32],
        credits: Some(0),
        cod: None,
        returnable: Some(true),
        takeable: Some(true),
        items: vec![AdminObservationItem {
            ordinal,
            kind: EnvelopeItemKind::Equipment,
            canonical_eid: Some("42".to_owned()),
            amount: None,
        }],
    }
}

fn active_detail() -> AdminDepositDetail {
    AdminDepositDetail {
        quarantine_id: QUARANTINE,
        version: 3,
        state: QuarantineState::Active,
        world: SettlementWorld::Main,
        external_mid: "12345".to_owned(),
        first_decision_id: fixture_uuid(0x90),
        reason: QuarantineReason::DuplicateRawEid,
        observation_total: 2,
        observations: vec![observation(1), observation(2)],
        allowed_actions: vec![AdminDepositAction::Recheck, AdminDepositAction::ReleaseQuarantine],
        final_decision: None,
    }
}

fn hold_receipt(command_id: CommandId) -> OperationsHoldReceipt {
    OperationsHoldReceipt::new(
        command_id,
        OperationsHoldDisposition::HoldSet,
        LISTING,
        DomainVersion::new(4).expect("版本"),
        ListingState::HeldOffMarket,
        Some(HOLD),
        Some(HeldOffMarketReason::OperationsHold),
    )
    .expect("止售回执")
}

fn closed_receipt(command_id: CommandId, listing_id: Uuid) -> OperationsHoldReceipt {
    OperationsHoldReceipt::new(
        command_id,
        OperationsHoldDisposition::Closed,
        listing_id,
        DomainVersion::new(4).expect("版本"),
        ListingState::Closed,
        None,
        None,
    )
    .expect("关闭回执")
}

fn release_receipt(command_id: CommandId, kind: ReleaseKind) -> ReleaseReceipt {
    let mut receipt = ReleaseReceipt::released(
        command_id,
        QUARANTINE,
        fixture_uuid(0xa0),
        fixture_uuid(0xa1),
        kind,
    );
    match kind {
        ReleaseKind::Accept => {
            receipt.listing_changes.push(ListingStateChange {
                listing_id: LISTING,
                listing_version: 4,
                state: ListingState::DepositAccepted,
            });
        }
        ReleaseKind::AuctionReturnCase => {
            receipt.return_mail_id = Some(fixture_uuid(0xa2));
            receipt.return_evidence_id = Some(fixture_uuid(0xa3));
        }
        ReleaseKind::AuctionRecoveryReturn => {
            receipt.obligation_id = Some(fixture_uuid(0xa4));
            receipt.return_mail_id = Some(fixture_uuid(0xa2));
            receipt.recovery_id = Some(fixture_uuid(0xa5));
            receipt.recovery_handoff_id = Some(fixture_uuid(0xa6));
        }
        ReleaseKind::AcquireReturn => {
            receipt.return_intake_id = Some(fixture_uuid(0xa7));
            receipt.original_round = Some(1);
            receipt.take_work_id = Some(fixture_uuid(0xa8));
            receipt.take_delivery_id = Some(fixture_uuid(0xa9));
        }
        ReleaseKind::Return | ReleaseKind::ArchiveEmpty => {}
    }
    receipt
}

fn final_decision(kind: ReleaseKind) -> auction_application::consignment::AdminFinalDecision {
    let mut decision = auction_application::consignment::AdminFinalDecision {
        release_id: fixture_uuid(0x100),
        decision_id: fixture_uuid(0x101),
        kind,
        owner: None,
        released_at: time(),
        listing_changes: Vec::new(),
        obligation_id: None,
        return_mail_id: None,
        return_evidence_id: None,
        recovery_id: None,
        recovery_handoff_id: None,
        return_intake_id: None,
        original_round: None,
        take_work_id: None,
        take_delivery_id: None,
    };
    match kind {
        ReleaseKind::Accept => decision.listing_changes.push(ListingStateChange {
            listing_id: LISTING,
            listing_version: 4,
            state: ListingState::DepositAccepted,
        }),
        ReleaseKind::Return | ReleaseKind::ArchiveEmpty => {}
        ReleaseKind::AcquireReturn => {
            decision.return_intake_id = Some(fixture_uuid(0x102));
            decision.original_round = Some(2);
            decision.take_work_id = Some(fixture_uuid(0x103));
            decision.take_delivery_id = Some(fixture_uuid(0x104));
        }
        ReleaseKind::AuctionReturnCase => {
            decision.return_mail_id = Some(fixture_uuid(0x105));
            decision.return_evidence_id = Some(fixture_uuid(0x106));
        }
        ReleaseKind::AuctionRecoveryReturn => {
            decision.obligation_id = Some(fixture_uuid(0x107));
            decision.return_mail_id = Some(fixture_uuid(0x105));
            decision.recovery_id = Some(fixture_uuid(0x108));
            decision.recovery_handoff_id = Some(fixture_uuid(0x109));
        }
    }
    decision
}

struct Backend {
    detail: Mutex<Option<AdminDepositDetail>>,
    rechecks: Mutex<HashMap<Uuid, (RecheckQuarantineCommand, RecheckReceipt)>>,
    releases: Mutex<HashMap<Uuid, (ReleaseQuarantineCommand, ReleaseReceipt)>>,
    holds: Mutex<HashMap<Uuid, (SetOperationsHoldCommand, OperationsHoldReceipt)>>,
    release_holds: Mutex<HashMap<Uuid, (ReleaseOperationsHoldCommand, OperationsHoldReceipt)>>,
    hold_failure: Mutex<Option<ConsignmentError>>,
    recheck_failure: Mutex<Option<ConsignmentError>>,
    release_failure: Mutex<Option<ConsignmentError>>,
    release_hold_failure: Mutex<Option<ConsignmentError>>,
    next_release_kind: Mutex<ReleaseKind>,
    listing_states: Mutex<HashMap<Uuid, ListingState>>,
    v1_versions: Mutex<HashMap<Uuid, u64>>,
    v1_calls: Mutex<Vec<Uuid>>,
    v2_records: Mutex<Vec<AdminListingRecordV2>>,
}

impl Default for Backend {
    fn default() -> Self {
        Self {
            detail: Mutex::new(Some(active_detail())),
            rechecks: Mutex::new(HashMap::new()),
            releases: Mutex::new(HashMap::new()),
            holds: Mutex::new(HashMap::new()),
            release_holds: Mutex::new(HashMap::new()),
            hold_failure: Mutex::new(None),
            recheck_failure: Mutex::new(None),
            release_failure: Mutex::new(None),
            release_hold_failure: Mutex::new(None),
            next_release_kind: Mutex::new(ReleaseKind::Return),
            listing_states: Mutex::new(HashMap::from([
                (LISTING, ListingState::Available),
                (UNMANAGED, ListingState::AwaitingDeposit),
                (SALE_PENDING, ListingState::SalePending),
                (WITHDRAWAL_PENDING, ListingState::WithdrawalPending),
                (CLOSED, ListingState::Closed),
            ])),
            v1_versions: Mutex::new(HashMap::from([(LISTING, 3), (UNMANAGED, 1)])),
            v1_calls: Mutex::new(Vec::new()),
            v2_records: Mutex::new(Vec::new()),
        }
    }
}

impl OwnedListingReadV2 for Backend {
    fn list_owned_v2(
        &self,
        _seller_user_id: UserId,
        _query: PrivateListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<PrivateListingV2>> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn owned_detail_v2(
        &self,
        _seller_user_id: UserId,
        _listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<PrivateListingV2>> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }
}

impl AdminListingReadV2 for Backend {
    fn list_admin_v2(
        &self,
        query: AdminListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<AdminListingRecordV2>> {
        let items = self.v2_records.lock().expect("v2 记录").clone();
        let total = items.len() as u64;
        Box::pin(async move { Ok(ListingReadPage { items, total, window: query.listings.window }) })
    }

    fn admin_detail_v2(
        &self,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<AdminListingDetailV2>> {
        let record = self
            .v2_records
            .lock()
            .expect("v2 记录")
            .iter()
            .find(|record| record.listing.listing_id == listing_id)
            .cloned();
        Box::pin(async move {
            Ok(record.map(|record| AdminListingDetailV2 {
                record,
                conflicts: Vec::new(),
                conflicts_truncated: false,
            }))
        })
    }
}

impl DepositRead for Backend {
    fn status(&self) -> ConsignmentFuture<'_, DepositStatus> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn instructions(
        &self,
        _seller: UserId,
        _listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<OwnedDepositInstructions>> {
        Box::pin(async { Ok(None) })
    }

    fn quarantine(
        &self,
        query: AdminDepositQuery,
    ) -> ConsignmentFuture<'_, Option<AdminDepositDetail>> {
        let value = self
            .detail
            .lock()
            .expect("隔离详情")
            .as_ref()
            .filter(|detail| detail.quarantine_id == query.quarantine_id)
            .cloned();
        Box::pin(async move { Ok(value) })
    }
}

impl ReservationStore for Backend {
    fn issue_reservation(
        &self,
        _command: IssueReservationCommand,
    ) -> ConsignmentFuture<'_, ReservationReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn expire_reservation(
        &self,
        _command: ExpireReservationCommand,
    ) -> ConsignmentFuture<'_, ReservationRetired> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn replay_issue_reservation(
        &self,
        _seller_user_id: UserId,
        _command_id: CommandId,
        _listing_id: Uuid,
        _request_hash: RequestHash,
    ) -> ConsignmentFuture<'_, Option<ReservationReceipt>> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }
}

impl CustodyControlStore for Backend {
    fn request_withdrawal(
        &self,
        _command: WithdrawDepositCommand,
    ) -> ConsignmentFuture<'_, WithdrawDepositReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn set_operations_hold(
        &self,
        command: SetOperationsHoldCommand,
    ) -> ConsignmentFuture<'_, OperationsHoldReceipt> {
        if let Some(error) = *self.hold_failure.lock().expect("止售失败") {
            return Box::pin(async move { Err(error) });
        }
        if command.listing_id == MISSING {
            return Box::pin(async {
                Err(ConsignmentError::Business(
                    auction_application::consignment::ListingBusinessRejection::ListingNotFound,
                ))
            });
        }
        let mut records = self.holds.lock().expect("止售账本");
        if let Some((first, receipt)) = records.get(&command.command_id.as_uuid()) {
            let replay = first.request_hash == command.request_hash;
            let receipt = receipt.clone();
            drop(records);
            return Box::pin(async move {
                if replay { Ok(receipt) } else { Err(ConsignmentError::CommandIdConflict) }
            });
        }
        let state =
            self.listing_states.lock().expect("挂单状态").get(&command.listing_id).copied().ok_or(
                ConsignmentError::Business(
                    auction_application::consignment::ListingBusinessRejection::ListingNotFound,
                ),
            );
        let state = match state {
            Ok(state) => state,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let receipt = match state {
            ListingState::Resolving | ListingState::Displayed | ListingState::AwaitingDeposit => {
                closed_receipt(command.command_id, command.listing_id)
            }
            ListingState::DepositAccepted
            | ListingState::Available
            | ListingState::HeldOffMarket => hold_receipt(command.command_id),
            _ => {
                return Box::pin(async {
                    Err(ConsignmentError::Deposit(DepositBusinessRejection::ListingNotActive))
                });
            }
        };
        records.insert(command.command_id.as_uuid(), (command, receipt.clone()));
        Box::pin(async move { Ok(receipt) })
    }

    fn release_operations_hold(
        &self,
        command: ReleaseOperationsHoldCommand,
    ) -> ConsignmentFuture<'_, OperationsHoldReceipt> {
        if let Some(error) = *self.release_hold_failure.lock().expect("解除止售失败") {
            return Box::pin(async move { Err(error) });
        }
        let mut records = self.release_holds.lock().expect("解除止售账本");
        if let Some((first, receipt)) = records.get(&command.command_id.as_uuid()) {
            let replay = first.request_hash == command.request_hash;
            let receipt = receipt.clone();
            drop(records);
            return Box::pin(async move {
                if replay { Ok(receipt) } else { Err(ConsignmentError::CommandIdConflict) }
            });
        }
        let receipt = OperationsHoldReceipt::new(
            command.command_id,
            OperationsHoldDisposition::HoldReleased,
            command.listing_id,
            DomainVersion::new(command.expected_version.value() + 1).expect("版本"),
            ListingState::HeldOffMarket,
            Some(command.hold_id),
            Some(HeldOffMarketReason::Review),
        )
        .expect("解除止售回执");
        records.insert(command.command_id.as_uuid(), (command, receipt.clone()));
        Box::pin(async move { Ok(receipt) })
    }

    fn replay_set_operations_hold(
        &self,
        admin_actor: String,
        command_id: CommandId,
        listing_id: Uuid,
        request_hash: RequestHash,
    ) -> ConsignmentFuture<'_, Option<OperationsHoldReceipt>> {
        let records = self.holds.lock().expect("止售账本");
        let result = match records.get(&command_id.as_uuid()) {
            None => Ok(None),
            Some((command, receipt))
                if command.admin_actor == admin_actor
                    && command.listing_id == listing_id
                    && command.request_hash == request_hash =>
            {
                Ok(Some(receipt.clone()))
            }
            Some(_) => Err(ConsignmentError::CommandIdConflict),
        };
        drop(records);
        Box::pin(async move { result })
    }
}

impl EnvelopeStore for Backend {
    fn decide_envelope(
        &self,
        _command: auction_application::consignment::DecideEnvelopeCommand,
    ) -> ConsignmentFuture<'_, auction_application::consignment::EnvelopeReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn append_quarantine_observation(
        &self,
        _command: auction_application::consignment::AppendQuarantineObservationCommand,
    ) -> ConsignmentFuture<'_, auction_application::consignment::ObservationReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn recheck_quarantine(
        &self,
        command: RecheckQuarantineCommand,
    ) -> ConsignmentFuture<'_, RecheckReceipt> {
        if let Some(error) = *self.recheck_failure.lock().expect("重检失败") {
            return Box::pin(async move { Err(error) });
        }
        let mut records = self.rechecks.lock().expect("重检账本");
        if let Some((first, receipt)) = records.get(&command.command_id.as_uuid()) {
            let replay = first.request_hash == command.request_hash;
            let receipt = receipt.clone();
            drop(records);
            return Box::pin(async move {
                if replay { Ok(receipt) } else { Err(ConsignmentError::CommandIdConflict) }
            });
        }
        let receipt =
            RecheckReceipt::queued(command.command_id, command.quarantine_id, fixture_uuid(0xb0));
        records.insert(command.command_id.as_uuid(), (command, receipt.clone()));
        Box::pin(async move { Ok(receipt) })
    }

    fn release_quarantine(
        &self,
        command: ReleaseQuarantineCommand,
    ) -> ConsignmentFuture<'_, ReleaseReceipt> {
        if let Some(error) = *self.release_failure.lock().expect("解除失败") {
            return Box::pin(async move { Err(error) });
        }
        let mut records = self.releases.lock().expect("解除账本");
        if let Some((first, receipt)) = records.get(&command.command_id.as_uuid()) {
            let replay = first.request_hash == command.request_hash;
            let receipt = receipt.clone();
            drop(records);
            return Box::pin(async move {
                if replay { Ok(receipt) } else { Err(ConsignmentError::CommandIdConflict) }
            });
        }
        let receipt =
            release_receipt(command.command_id, *self.next_release_kind.lock().expect("解除种类"));
        records.insert(command.command_id.as_uuid(), (command, receipt.clone()));
        Box::pin(async move { Ok(receipt) })
    }
}

impl DispositionStore for Backend {
    fn start_disposition_write(
        &self,
        _command: StartDispositionWriteCommand,
    ) -> ConsignmentFuture<'_, WriteStartOutcome> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn apply_mail_proof(
        &self,
        _command: ApplyMailProofCommand,
    ) -> ConsignmentFuture<'_, auction_application::consignment::ProofReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }
}

impl SeasonStore for Backend {
    fn queue_season_notice(
        &self,
        _command: SeasonNoticeCommand,
    ) -> ConsignmentFuture<'_, SeasonNoticeReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn close_season(
        &self,
        _command: SeasonCloseCommand,
    ) -> ConsignmentFuture<'_, SeasonCloseReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }
}

impl ListingCommandStore for Backend {
    fn register<'a>(
        &'a self,
        _command: RegisterListingCommand<'a>,
        _sealer: Option<&'a dyn auction_application::admin::EquipmentKeySealerPort>,
        _replay_crypto: Option<&'a dyn auction_application::consignment::RegistrationReplayCrypto>,
    ) -> ConsignmentFuture<'a, ListingCommandOutcome<ListingRegistered>> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn reprice(
        &self,
        _command: auction_application::consignment::RepriceListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingPriceReceipt>> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn withdraw(
        &self,
        _command: auction_application::consignment::WithdrawListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn apply_lookup(
        &self,
        _command: ApplyListingLookupCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingLookupEffect>> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn auto_close(
        &self,
        _command: AutoCloseListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingAutoCloseEffect>> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn admin_delist(
        &self,
        command: auction_application::consignment::AdminDelistListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
        self.v1_calls.lock().expect("v1 端口").push(command.listing_id);
        let state = self
            .listing_states
            .lock()
            .expect("挂单状态")
            .get(&command.listing_id)
            .copied()
            .unwrap_or(ListingState::Resolving);
        if state.is_custody_managed() {
            return Box::pin(async move {
                Err(ConsignmentError::from(ListingError::InvalidTransition {
                    from: "available",
                    action: "admin_delist",
                }))
            });
        }
        let next_version = {
            let mut versions = self.v1_versions.lock().expect("v1 版本");
            let version = versions.entry(command.listing_id).or_insert(1);
            *version += 1;
            *version
        };
        Box::pin(async move {
            let closed = ListingClosed::new(
                command.listing_id,
                DomainVersion::new(next_version).expect("版本"),
                None,
            )?;
            Ok(ListingCommandOutcome::Accepted(closed))
        })
    }
}

#[derive(Default)]
struct Sessions;

impl Sessions {
    fn authenticate(
        &self,
        token: String,
        csrf: String,
        now: Timestamp,
    ) -> Result<AdminSessionIdentity, SessionError> {
        if token != "session-placeholder" || csrf != "csrf-placeholder" {
            return Err(SessionError::Unauthenticated);
        }
        Ok(AdminSessionIdentity {
            actor: ACTOR.to_owned(),
            expires_at: now,
            absolute_expires_at: now,
        })
    }
}

impl AdminSessionPort for Sessions {
    fn login(
        &self,
        _actor: String,
        _secret: Vec<u8>,
        _now: Timestamp,
    ) -> auction_application::core::ApplicationFuture<'_, AdminLoginResult> {
        Box::pin(async { Err(SessionError::Unauthenticated.into()) })
    }

    fn current(
        &self,
        token: String,
        csrf: String,
        now: Timestamp,
    ) -> auction_application::core::ApplicationFuture<'_, AdminSessionIdentity> {
        let result = self.authenticate(token, csrf, now).map_err(Into::into);
        Box::pin(async move { result })
    }

    fn touch(
        &self,
        token: String,
        csrf: String,
        now: Timestamp,
    ) -> auction_application::core::ApplicationFuture<'_, AdminSessionIdentity> {
        let result = self.authenticate(token, csrf, now).map_err(Into::into);
        Box::pin(async move { result })
    }

    fn logout(
        &self,
        _token: String,
        _now: Timestamp,
    ) -> auction_application::core::ApplicationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Clone)]
struct TestState {
    admin: AdminApiState,
    deposit_caps: AdminDepositCapabilities,
    manage: Arc<ManageListingService>,
}

impl AdminConsignmentState for TestState {
    fn admin_state(&self) -> &AdminApiState {
        &self.admin
    }

    fn consignment_read(&self) -> Option<&dyn AdminListingRead> {
        None
    }

    fn consignment_manage(&self) -> Option<&ManageListingService> {
        Some(self.manage.as_ref())
    }
}

impl AdminDepositState for TestState {
    fn deposit(&self) -> &AdminDepositCapabilities {
        &self.deposit_caps
    }
}

fn setup(backend: &Arc<Backend>, enabled: bool) -> TestState {
    let security = AdminHttpSecurity::new(HttpSecurityOptions {
        cookie_secure: true,
        origins_enabled: true,
        allowed_origins: &[ORIGIN.to_owned()],
        login_limit: 10,
        mutation_limit: 100,
        anonymous_read_limit: 10,
        anonymous_read_concurrency: 2,
        rate_limit_window: Duration::from_secs(60),
        request_timeout: Duration::from_secs(15),
        trusted_proxy_cidrs: &[],
        trusted_unix_proxy: false,
    })
    .expect("Admin 安全策略");
    let session: Arc<dyn AdminSessionPort> = Arc::new(Sessions);
    let admin = AdminApiState::new(
        Some(session),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        security,
    );
    let service = Arc::new(DepositCommandService::new(
        backend.clone(),
        backend.clone(),
        backend.clone(),
        backend.clone(),
        backend.clone(),
    ));
    let manage = Arc::new(ManageListingService::new(backend.clone(), true));
    TestState {
        admin,
        deposit_caps: AdminDepositCapabilities {
            enabled,
            read: Some(backend.clone()),
            commands: Some(service),
        },
        manage,
    }
}

fn router(state: TestState) -> Router {
    mount_v1_admin(mount(Router::new())).with_state(state)
}

fn headers() -> HeaderMap {
    HeaderMap::from_iter([
        (header::ORIGIN, HeaderValue::from_static(ORIGIN)),
        (header::CONTENT_TYPE, HeaderValue::from_static("application/json")),
        (
            header::COOKIE,
            HeaderValue::from_static(
                "auction_admin_session=session-placeholder; auction_admin_csrf=csrf-placeholder",
            ),
        ),
        (ADMIN_CSRF_HEADER.parse().expect("CSRF 头"), HeaderValue::from_static("csrf-placeholder")),
    ])
}

fn request(method: Method, uri: &str, body: Body) -> Request<Body> {
    let mut request = Request::builder().method(method).uri(uri).body(body).expect("HTTP 请求");
    *request.headers_mut() = headers();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 23], 3124))));
    request
}

fn get(uri: &str) -> Request<Body> {
    request(Method::GET, uri, Body::empty())
}

fn post(uri: &str, body: Value) -> Request<Body> {
    request(Method::POST, uri, Body::from(body.to_string()))
}

async fn send(app: &Router, request: Request<Body>) -> Response {
    app.clone().oneshot(request).await.expect("HTTP 响应")
}

async fn json(response: Response, status: StatusCode) -> Value {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(response.headers().get(header::ETAG).is_none());
    let bytes = to_bytes(response.into_body(), 1 << 20).await.expect("HTTP 正文");
    serde_json::from_slice(&bytes).expect("HTTP JSON")
}

async fn failure(response: Response, status: StatusCode, code: &str) -> Value {
    let body = json(response, status).await;
    assert_eq!(body["code"], code);
    assert!(body["data"].is_null());
    assert!(body.get("request_id").is_some());
    body
}

#[tokio::test]
async fn detail_returns_safe_observation_projection_and_total() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, true));
    let response = send(&app, get(&format!("{DETAIL}/{QUARANTINE}"))).await;
    let body = json(response, StatusCode::OK).await;
    assert_eq!(body["data"]["quarantine_id"], QUARANTINE.to_string());
    assert_eq!(body["data"]["observation_total"], 2);
    assert_eq!(body["data"]["observations"].as_array().map(Vec::len), Some(2));
    assert_eq!(body["data"]["observations"][0]["items"][0]["external_equipment_id"], "42");
    assert_eq!(body["data"]["allowed_actions"], json!(["recheck", "release"]));
    assert_eq!(body["data"]["release"], Value::Null);
    assert!(!body.to_string().contains("FixtureKey"));
}

#[tokio::test]
async fn detail_reports_cumulative_total_and_cash_observation_items() {
    let backend = Arc::new(Backend::default());
    {
        let mut detail = backend.detail.lock().expect("隔离详情");
        let detail = detail.as_mut().expect("隔离详情");
        detail.observation_total = 7;
        detail.observations[0].credits = Some(150);
        detail.observations[0].cod = Some(0);
        detail.observations[0].returnable = Some(false);
        detail.observations[0].takeable = Some(true);
        detail.observations[0].items = vec![
            AdminObservationItem {
                ordinal: 1,
                kind: EnvelopeItemKind::Equipment,
                canonical_eid: Some("42".to_owned()),
                amount: None,
            },
            AdminObservationItem {
                ordinal: 2,
                kind: EnvelopeItemKind::Credits,
                canonical_eid: None,
                amount: Some(150),
            },
            AdminObservationItem {
                ordinal: 3,
                kind: EnvelopeItemKind::Cod,
                canonical_eid: None,
                amount: Some(0),
            },
        ];
    }
    let app = router(setup(&backend, true));
    let body = json(send(&app, get(&format!("{DETAIL}/{QUARANTINE}"))).await, StatusCode::OK).await;
    assert_eq!(body["data"]["observation_total"], 7);
    assert_eq!(body["data"]["observations"].as_array().map(Vec::len), Some(2));
    assert_eq!(body["data"]["observations"][0]["credits"], 150);
    assert_eq!(body["data"]["observations"][0]["cod"], 0);
    assert_eq!(body["data"]["observations"][0]["returnable"], false);
    assert_eq!(body["data"]["observations"][0]["takeable"], true);
    assert_eq!(body["data"]["observations"][0]["items"][1]["kind"], "credits");
    assert_eq!(body["data"]["observations"][0]["items"][1]["amount"], 150);
    assert_eq!(body["data"]["observations"][0]["items"][2]["kind"], "cod");
    assert_eq!(body["data"]["observations"][0]["items"][2]["amount"], 0);
}

#[tokio::test]
async fn released_detail_covers_all_six_release_branches() {
    for (kind, wire) in [
        (ReleaseKind::Accept, "accept"),
        (ReleaseKind::Return, "return"),
        (ReleaseKind::ArchiveEmpty, "archive_empty"),
        (ReleaseKind::AcquireReturn, "acquire_return"),
        (ReleaseKind::AuctionReturnCase, "auction_return_case"),
        (ReleaseKind::AuctionRecoveryReturn, "auction_recovery_return"),
    ] {
        let backend = Arc::new(Backend::default());
        {
            let mut detail = backend.detail.lock().expect("隔离详情");
            let detail = detail.as_mut().expect("隔离详情");
            detail.state = QuarantineState::Released;
            detail.allowed_actions.clear();
            detail.final_decision = Some(final_decision(kind));
        }
        let app = router(setup(&backend, true));
        let body =
            json(send(&app, get(&format!("{DETAIL}/{QUARANTINE}"))).await, StatusCode::OK).await;
        assert_eq!(body["data"]["release"]["release_id"], fixture_uuid(0x100).to_string());
        assert_eq!(body["data"]["release"]["decision_id"], fixture_uuid(0x101).to_string());
        assert_eq!(body["data"]["release"]["outcome"]["decision_kind"], wire);
        match kind {
            ReleaseKind::Accept => {
                assert_eq!(
                    body["data"]["release"]["outcome"]["listing_changes"][0]["listing_id"],
                    LISTING.to_string()
                );
            }
            ReleaseKind::Return | ReleaseKind::ArchiveEmpty => {}
            ReleaseKind::AcquireReturn => {
                assert_eq!(
                    body["data"]["release"]["outcome"]["return_intake_id"],
                    fixture_uuid(0x102).to_string()
                );
                assert_eq!(body["data"]["release"]["outcome"]["original_round"], 2);
            }
            ReleaseKind::AuctionReturnCase => {
                assert_eq!(
                    body["data"]["release"]["outcome"]["return_evidence_id"],
                    fixture_uuid(0x106).to_string()
                );
                assert_eq!(body["data"]["release"]["outcome"]["obligation_id"], Value::Null);
            }
            ReleaseKind::AuctionRecoveryReturn => {
                assert_eq!(
                    body["data"]["release"]["outcome"]["recovery_handoff_id"],
                    fixture_uuid(0x109).to_string()
                );
            }
        }
    }
}

#[tokio::test]
async fn missing_quarantine_and_bad_query_use_safe_404_and_400() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, true));
    let missing = send(&app, get(&format!("{DETAIL}/{MISSING}"))).await;
    failure(missing, StatusCode::NOT_FOUND, "listing_not_found").await;
    for path in [
        format!("{DETAIL}/{QUARANTINE}?limit=0"),
        format!("{DETAIL}/{QUARANTINE}?limit=201"),
        format!("{DETAIL}/{QUARANTINE}?cursor=not-a-uuid"),
    ] {
        let response = send(&app, get(&path)).await;
        failure(response, StatusCode::BAD_REQUEST, "invalid_request").await;
    }
}

#[tokio::test]
async fn recheck_returns_202_and_replays_the_first_work() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, true));
    let body = json!({"command_id": COMMAND, "expected_version": 3});
    let first = send(&app, post(&format!("{DETAIL}/{QUARANTINE}/recheck"), body.clone())).await;
    let first = json(first, StatusCode::ACCEPTED).await;
    assert_eq!(first["data"]["operation"], "recheck_quarantine");
    assert_eq!(first["data"]["disposition"], "recheck_queued");

    let replay = send(&app, post(&format!("{DETAIL}/{QUARANTINE}/recheck"), body)).await;
    let replay = json(replay, StatusCode::ACCEPTED).await;
    assert_eq!(replay["data"], first["data"]);

    let conflict = send(
        &app,
        post(
            &format!("{DETAIL}/{QUARANTINE}/recheck"),
            json!({"command_id": COMMAND, "expected_version": 4}),
        ),
    )
    .await;
    failure(conflict, StatusCode::CONFLICT, "command_id_conflict").await;
}

#[tokio::test]
async fn release_covers_all_six_branches() {
    let kinds = [
        ("accept", ReleaseKind::Accept, "accept"),
        ("return", ReleaseKind::Return, "return"),
        ("archive_empty", ReleaseKind::ArchiveEmpty, "archive_empty"),
        ("acquire_return", ReleaseKind::AcquireReturn, "acquire_return"),
        ("auction_return_case", ReleaseKind::AuctionReturnCase, "auction_return_case"),
        ("auction_recovery_return", ReleaseKind::AuctionRecoveryReturn, "auction_recovery_return"),
    ];
    for (label, kind, wire) in kinds {
        let backend = Arc::new(Backend::default());
        *backend.next_release_kind.lock().expect("解除种类") = kind;
        let app = router(setup(&backend, true));
        let response = send(
            &app,
            post(
                &format!("{DETAIL}/{QUARANTINE}/release-quarantine"),
                json!({
                    "command_id": COMMAND,
                    "expected_version": 3,
                    "observation_id": fixture_uuid(0x81),
                    "proposed_kind": wire,
                }),
            ),
        )
        .await;
        let body = json(response, StatusCode::OK).await;
        assert_eq!(body["data"]["decision_kind"], label);
        match kind {
            ReleaseKind::Accept => {
                assert_eq!(body["data"]["listing_changes"][0]["state"], "deposit_accepted");
            }
            ReleaseKind::Return | ReleaseKind::ArchiveEmpty => {}
            ReleaseKind::AcquireReturn => {
                assert_eq!(body["data"]["original_round"], 1);
                assert_eq!(body["data"]["take_delivery_id"], fixture_uuid(0xa9).to_string());
            }
            ReleaseKind::AuctionReturnCase => {
                assert_eq!(body["data"]["obligation_id"], Value::Null);
                assert_eq!(body["data"]["return_mail_id"], fixture_uuid(0xa2).to_string());
            }
            ReleaseKind::AuctionRecoveryReturn => {
                assert_eq!(body["data"]["recovery_id"], fixture_uuid(0xa5).to_string());
                assert_eq!(body["data"]["recovery_handoff_id"], fixture_uuid(0xa6).to_string());
            }
        }
    }
}

#[tokio::test]
async fn release_rejects_force_and_unknown_fields_before_business() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, true));
    let response = send(
        &app,
        post(
            &format!("{DETAIL}/{QUARANTINE}/release-quarantine"),
            json!({
                "command_id": COMMAND,
                "expected_version": 3,
                "observation_id": fixture_uuid(0x81),
                "proposed_kind": "return",
                "force": true,
            }),
        ),
    )
    .await;
    failure(response, StatusCode::BAD_REQUEST, "invalid_request").await;
    assert!(backend.releases.lock().expect("解除账本").is_empty());
}

#[tokio::test]
async fn delist_closes_unmanaged_holds_managed_and_rejects_inactive_states() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, true));
    let managed = send(
        &app,
        post(
            &format!("{LISTINGS}/{LISTING}/delist"),
            json!({"command_id": COMMAND, "expected_version": 3, "reason": REASON}),
        ),
    )
    .await;
    let managed = json(managed, StatusCode::OK).await;
    assert_eq!(managed["data"]["disposition"], "hold_set");
    assert_eq!(managed["data"]["state"], "held_off_market");
    assert_eq!(managed["data"]["hold_id"], HOLD.to_string());
    assert!(backend.v1_calls.lock().expect("v1 端口").is_empty());

    let unmanaged = send(
        &app,
        post(
            &format!("{LISTINGS}/{UNMANAGED}/delist"),
            json!({"command_id": OTHER_COMMAND, "expected_version": 3, "reason": REASON}),
        ),
    )
    .await;
    let unmanaged = json(unmanaged, StatusCode::OK).await;
    assert_eq!(unmanaged["data"]["disposition"], "closed");
    assert_eq!(unmanaged["data"]["operation"], "admin_delist");
    assert_eq!(unmanaged["data"]["state"], "closed");
    assert_eq!(unmanaged["data"]["schema_version"], 2);
    assert!(unmanaged["data"].get("hold_id").is_none());
    assert!(backend.v1_calls.lock().expect("v1 端口").is_empty());

    for (offset, listing_id) in [SALE_PENDING, WITHDRAWAL_PENDING, CLOSED].into_iter().enumerate() {
        let response = send(
            &app,
            post(
                &format!("{LISTINGS}/{listing_id}/delist"),
                json!({
                    "command_id": fixture_uuid(0x62 + offset as u128),
                    "expected_version": 3,
                    "reason": REASON,
                }),
            ),
        )
        .await;
        failure(response, StatusCode::CONFLICT, "listing_not_active").await;
    }
    assert!(backend.v1_calls.lock().expect("v1 端口").is_empty());
}

#[tokio::test]
async fn delist_replays_the_first_receipt_after_soft_close() {
    let backend = Arc::new(Backend::default());
    let enabled = router(setup(&backend, true));
    let body = json!({"command_id": COMMAND, "expected_version": 3, "reason": REASON});
    let first = send(&enabled, post(&format!("{LISTINGS}/{LISTING}/delist"), body.clone())).await;
    let first = json(first, StatusCode::OK).await;
    assert_eq!(first["data"]["disposition"], "hold_set");
    let holds = backend.holds.lock().expect("止售账本").len();

    // 不变量 1、D9 §6.3：软关闭只截止新承诺；同一命令重放仍返回首次回执。
    let disabled = router(setup(&backend, false));
    let reply = send(&disabled, post(&format!("{LISTINGS}/{LISTING}/delist"), body)).await;
    let reply = json(reply, StatusCode::OK).await;
    assert_eq!(reply["data"], first["data"]);

    let conflict = send(
        &disabled,
        post(
            &format!("{LISTINGS}/{LISTING}/delist"),
            json!({"command_id": COMMAND, "expected_version": 4, "reason": REASON}),
        ),
    )
    .await;
    failure(conflict, StatusCode::CONFLICT, "command_id_conflict").await;

    let fresh = send(
        &disabled,
        post(
            &format!("{LISTINGS}/{LISTING}/delist"),
            json!({"command_id": OTHER_COMMAND, "expected_version": 3, "reason": REASON}),
        ),
    )
    .await;
    failure(fresh, StatusCode::NOT_FOUND, "consignment_disabled").await;
    assert_eq!(
        backend.holds.lock().expect("止售账本").len(),
        holds,
        "关闭后的重放与新 ID 都不得写入新命令"
    );
}

#[tokio::test]
async fn v1_admin_delist_on_managed_listing_is_409_and_keeps_version() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, true));
    let response = send(
        &app,
        post(
            &format!("/api/v1/admin/consignment/listings/{LISTING}/delist"),
            json!({"command_id": COMMAND, "expected_version": 3, "reason": REASON}),
        ),
    )
    .await;
    failure(response, StatusCode::CONFLICT, "listing_not_active").await;
    assert_eq!(backend.v1_versions.lock().expect("v1 版本")[&LISTING], 3);
    assert_eq!(backend.v1_calls.lock().expect("v1 端口").as_slice(), &[LISTING]);
}

#[tokio::test]
async fn release_hold_returns_released_receipt_and_maps_mismatch() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, true));
    let response = send(
        &app,
        post(
            &format!("{LISTINGS}/{LISTING}/release-hold"),
            json!({"command_id": COMMAND, "expected_version": 3, "hold_id": HOLD}),
        ),
    )
    .await;
    let body = json(response, StatusCode::OK).await;
    assert_eq!(body["data"]["operation"], "release_hold");
    assert_eq!(body["data"]["disposition"], "hold_released");
    assert_eq!(body["data"]["state"], "held_off_market");

    *backend.release_hold_failure.lock().expect("解除止售失败") =
        Some(ConsignmentError::Deposit(DepositBusinessRejection::HoldMismatch));
    let failed = send(
        &app,
        post(
            &format!("{LISTINGS}/{LISTING}/release-hold"),
            json!({"command_id": OTHER_COMMAND, "expected_version": 3, "hold_id": HOLD}),
        ),
    )
    .await;
    failure(failed, StatusCode::CONFLICT, "hold_mismatch").await;
}

#[tokio::test]
async fn delist_and_quarantine_failures_keep_permission_and_quota_codes() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, true));
    for (error, status, code) in [
        (
            ConsignmentError::Business(ListingBusinessRejection::ParticipantIneligible),
            StatusCode::FORBIDDEN,
            "participant_ineligible",
        ),
        (
            ConsignmentError::Business(ListingBusinessRejection::RegistrationRateLimited),
            StatusCode::TOO_MANY_REQUESTS,
            "registration_rate_limited",
        ),
        (
            ConsignmentError::Business(ListingBusinessRejection::ListingLimitReached),
            StatusCode::TOO_MANY_REQUESTS,
            "listing_limit_reached",
        ),
    ] {
        *backend.hold_failure.lock().expect("止售失败") = Some(error);
        let response = send(
            &app,
            post(
                &format!("{LISTINGS}/{LISTING}/delist"),
                json!({"command_id": OTHER_COMMAND, "expected_version": 3, "reason": REASON}),
            ),
        )
        .await;
        failure(response, status, code).await;
    }

    *backend.recheck_failure.lock().expect("重检失败") =
        Some(ConsignmentError::Deposit(DepositBusinessRejection::MailUnavailable));
    let mail = send(
        &app,
        post(
            &format!("{DETAIL}/{QUARANTINE}/recheck"),
            json!({"command_id": COMMAND, "expected_version": 3}),
        ),
    )
    .await;
    failure(mail, StatusCode::SERVICE_UNAVAILABLE, "mail_unavailable").await;

    *backend.recheck_failure.lock().expect("重检失败") = Some(ConsignmentError::OutcomeUnknown);
    let unknown = send(
        &app,
        post(
            &format!("{DETAIL}/{QUARANTINE}/recheck"),
            json!({"command_id": COMMAND, "expected_version": 3}),
        ),
    )
    .await;
    failure(unknown, StatusCode::SERVICE_UNAVAILABLE, "outcome_unknown").await;

    *backend.release_failure.lock().expect("解除失败") =
        Some(ConsignmentError::Deposit(DepositBusinessRejection::SourceMismatch));
    let source_mismatch = send(
        &app,
        post(
            &format!("{DETAIL}/{QUARANTINE}/release-quarantine"),
            json!({
                "command_id": COMMAND,
                "expected_version": 3,
                "observation_id": fixture_uuid(0x81),
                "proposed_kind": "return",
            }),
        ),
    )
    .await;
    failure(source_mismatch, StatusCode::CONFLICT, "source_mismatch").await;

    *backend.release_failure.lock().expect("解除失败") =
        Some(ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed));
    let action_not_allowed = send(
        &app,
        post(
            &format!("{DETAIL}/{QUARANTINE}/release-quarantine"),
            json!({
                "command_id": OTHER_COMMAND,
                "expected_version": 3,
                "observation_id": fixture_uuid(0x82),
                "proposed_kind": "accept",
            }),
        ),
    )
    .await;
    failure(action_not_allowed, StatusCode::FORBIDDEN, "action_not_allowed").await;
}

#[tokio::test]
async fn quarantine_detail_fails_closed_on_illegal_read_port_shapes() {
    for mutation in [
        "cash_without_amount",
        "released_without_decision",
        "released_without_release_id",
        "return_case_without_evidence",
        "total_smaller_than_page",
    ] {
        let backend = Arc::new(Backend::default());
        {
            let mut detail = backend.detail.lock().expect("隔离详情");
            let detail = detail.as_mut().expect("隔离详情");
            match mutation {
                "cash_without_amount" => {
                    detail.observations[0].items = vec![AdminObservationItem {
                        ordinal: 1,
                        kind: EnvelopeItemKind::Credits,
                        canonical_eid: None,
                        amount: None,
                    }];
                }
                "released_without_decision" => {
                    detail.state = QuarantineState::Released;
                    detail.allowed_actions.clear();
                }
                "released_without_release_id" => {
                    detail.state = QuarantineState::Released;
                    detail.allowed_actions.clear();
                    let mut decision = final_decision(ReleaseKind::Return);
                    decision.release_id = Uuid::nil();
                    detail.final_decision = Some(decision);
                }
                "return_case_without_evidence" => {
                    detail.state = QuarantineState::Released;
                    detail.allowed_actions.clear();
                    let mut decision = final_decision(ReleaseKind::AuctionReturnCase);
                    decision.return_evidence_id = None;
                    detail.final_decision = Some(decision);
                }
                "total_smaller_than_page" => detail.observation_total = 1,
                _ => unreachable!("测试变体"),
            }
        }
        let app = router(setup(&backend, true));
        let response = send(&app, get(&format!("{DETAIL}/{QUARANTINE}"))).await;
        failure(response, StatusCode::SERVICE_UNAVAILABLE, "consignment_unavailable").await;
    }
}

#[tokio::test]
async fn admin_v2_routes_enforce_csrf_methods_and_continuation_while_disabled() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, false));
    let mut no_csrf = get(&format!("{DETAIL}/{QUARANTINE}"));
    no_csrf.headers_mut().remove(ADMIN_CSRF_HEADER);
    let response = send(&app, no_csrf).await;
    json(response, StatusCode::OK).await;

    let wrong_method = send(&app, post(&format!("{DETAIL}/{QUARANTINE}"), json!({}))).await;
    assert_eq!(wrong_method.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(wrong_method.headers()[header::CACHE_CONTROL], "no-store");

    let mut no_csrf = post(
        &format!("{DETAIL}/{QUARANTINE}/recheck"),
        json!({"command_id": COMMAND, "expected_version": 3}),
    );
    no_csrf.headers_mut().remove(ADMIN_CSRF_HEADER);
    failure(send(&app, no_csrf).await, StatusCode::FORBIDDEN, "csrf_invalid").await;

    let mut bad_origin = post(
        &format!("{DETAIL}/{QUARANTINE}/recheck"),
        json!({"command_id": COMMAND, "expected_version": 3}),
    );
    bad_origin
        .headers_mut()
        .insert(header::ORIGIN, HeaderValue::from_static("https://other.example.invalid"));
    failure(send(&app, bad_origin).await, StatusCode::FORBIDDEN, "origin_forbidden").await;

    let recheck = send(
        &app,
        post(
            &format!("{DETAIL}/{QUARANTINE}/recheck"),
            json!({"command_id": COMMAND, "expected_version": 3}),
        ),
    )
    .await;
    json(recheck, StatusCode::ACCEPTED).await;

    let release = send(
        &app,
        post(
            &format!("{DETAIL}/{QUARANTINE}/release-quarantine"),
            json!({
                "command_id": COMMAND,
                "expected_version": 3,
                "observation_id": fixture_uuid(0x81),
                "proposed_kind": "return",
            }),
        ),
    )
    .await;
    json(release, StatusCode::OK).await;

    let release_hold = send(
        &app,
        post(
            &format!("{LISTINGS}/{LISTING}/release-hold"),
            json!({"command_id": OTHER_COMMAND, "expected_version": 3, "hold_id": HOLD}),
        ),
    )
    .await;
    json(release_hold, StatusCode::OK).await;

    let hold = send(
        &app,
        post(
            &format!("{LISTINGS}/{LISTING}/delist"),
            json!({"command_id": OTHER_COMMAND, "expected_version": 3, "reason": REASON}),
        ),
    )
    .await;
    failure(hold, StatusCode::NOT_FOUND, "consignment_disabled").await;
}

#[tokio::test]
async fn admin_detail_without_session_is_401_before_business() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, true));
    let mut request = get(&format!("{DETAIL}/{QUARANTINE}"));
    request.headers_mut().remove(header::COOKIE);
    let response = send(&app, request).await;
    failure(response, StatusCode::UNAUTHORIZED, "unauthenticated").await;
}

fn admin_record(listing: PrivateListingV2) -> AdminListingRecordV2 {
    AdminListingRecordV2 {
        listing,
        seller_user_id: UserId::from_uuid(fixture_uuid(0xfa)),
        seller_name: "FixtureSeller".to_owned(),
        admin_delist_reason: None,
    }
}

#[tokio::test]
async fn v2_listing_reads_show_custody_hold_and_admin_actions_not_seller_actions() {
    let backend = Arc::new(Backend::default());
    {
        let mut records = backend.v2_records.lock().expect("v2 记录");
        records.push(admin_record(crate::mapping::fixtures::held_by_operations(LISTING, HOLD)));
        records.push(admin_record(crate::mapping::fixtures::available(UNMANAGED)));
    }
    let app = router(setup(&backend, true));

    let page = json(send(&app, get(LISTINGS)).await, StatusCode::OK).await;
    assert_eq!(page["data"]["total"], 2);
    let held = &page["data"]["items"][0];
    assert_eq!(held["hold"]["hold_id"], HOLD.to_string());
    // hold_id 只在 Admin 的 hold 展示，本人视图（含 Admin 内嵌的）不带。
    assert!(held["listing"]["view"]["hold_id"].is_null());
    assert_eq!(held["admin_actions"], json!(["release_hold"]));
    assert_eq!(held["position"]["state"], "in_custody");
    assert_eq!(held["position"]["generation"], 1);
    assert_eq!(held["seller_name"], "FixtureSeller");
    // Admin 视图不带卖家动作，卖家动作与 Admin 动作互不串味。
    assert_eq!(held["listing"]["allowed_actions"], json!([]));
    let free = &page["data"]["items"][1];
    assert_eq!(free["hold"], Value::Null);
    assert_eq!(free["admin_actions"], json!(["delist"]));

    let detail =
        json(send(&app, get(&format!("{LISTINGS}/{LISTING}"))).await, StatusCode::OK).await;
    assert_eq!(detail["data"]["record"], *held);
    assert_eq!(detail["data"]["conflicts"], json!([]));
    assert!(!detail.to_string().contains("FixtureKey"));

    let missing = send(&app, get(&format!("{LISTINGS}/{MISSING}"))).await;
    failure(missing, StatusCode::NOT_FOUND, "listing_not_found").await;
}

/// 不变量 1、D9 §6.3：软关闭后解除既有止售照常，新的下架仍被拒。
#[tokio::test]
async fn release_hold_continues_after_soft_close_while_new_delist_is_refused() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, false));
    let response = send(
        &app,
        post(
            &format!("{LISTINGS}/{LISTING}/release-hold"),
            json!({"command_id": COMMAND, "expected_version": 3, "hold_id": HOLD}),
        ),
    )
    .await;
    let body = json(response, StatusCode::OK).await;
    assert_eq!(body["data"]["disposition"], "hold_released");

    let delist = send(
        &app,
        post(
            &format!("{LISTINGS}/{LISTING}/delist"),
            json!({"command_id": OTHER_COMMAND, "expected_version": 3, "reason": "fixture"}),
        ),
    )
    .await;
    failure(delist, StatusCode::NOT_FOUND, "consignment_disabled").await;
}
