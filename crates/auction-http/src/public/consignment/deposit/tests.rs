//! Public v2 处理器的离线路由测试：真实 Axum 守卫、严格 DTO 与内存端口。

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::consignment::{
    ActiveDepositInstructions, AdminDelistListingCommand, AdminDepositDetail, AdminDepositQuery,
    AppendQuarantineObservationCommand, ApplyListingLookupCommand, ApplyMailProofCommand,
    AutoCloseListingCommand, ConsignmentError, ConsignmentFuture, CustodyControlStore,
    DecideEnvelopeCommand, DepositCommandService, DepositRead, DepositStatus, DepositWorldStatus,
    DepositWorlds, DispositionStore, EnvelopeReceipt, EnvelopeStore, IssueReservationCommand,
    ListingBusinessRejection, ListingClosed, ListingCommandOutcome, ListingCommandStore,
    ListingPriceReceipt, ListingRegistered, ManageListingService, ObservationReceipt,
    OwnedDepositInstructions, RecheckQuarantineCommand, RecheckReceipt, RegisterListingCommand,
    RegistrationReplayCrypto, ReleaseQuarantineCommand, ReleaseReceipt, RepriceListingCommand,
    ReservationReceipt, ReservationStore, SeasonCloseCommand, SeasonCloseReceipt,
    SeasonNoticeCommand, SeasonNoticeReceipt, SeasonStore, StartDispositionWriteCommand,
    WithdrawDepositCommand, WithdrawDepositReceipt, WithdrawDisposition, WithdrawListingCommand,
    WriteStartOutcome,
};
use auction_application::core::{ApplicationFuture, RequestHash, SessionError};
use auction_application::public::{
    PublicLoginResult, PublicSessionIdentity, PublicSessionPort, PublicSessionPreferenceResult,
    SetPublicSessionPreferenceCommand,
};
use auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER;
use auction_domain::consignment::deposit::format_token;
use auction_domain::consignment::{
    FeeBps, ListingPrice, ListingState, ListingVersion, SeasonState,
};
use auction_domain::{CommandId, SettlementWorld, UserId};
use auction_time::Timestamp;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, HeaderValue, Method, Request, header};
use axum::middleware;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::{DepositCapabilities, DepositState, mount};
use crate::middleware::boundary::{HttpBoundary, enforce};
use crate::middleware::security::{HttpSecurityOptions, PublicHttpSecurity};
use crate::public::consignment::{ConsignmentCapabilities, ConsignmentState, mount as mount_v1};
use crate::state::PublicApiState;

const STATUS: &str = "/api/v2/consignment/status";
const OWNED: &str = "/api/v2/me/consignment/listings";
const WRITE: &str = "/api/v2/consignment/listings";
const LISTING: Uuid = fixture_uuid(1);
const OTHER_LISTING: Uuid = fixture_uuid(2);
const UNMANAGED: Uuid = fixture_uuid(3);
const SELLER: Uuid = fixture_uuid(0xfa);
const OTHER_SELLER: Uuid = fixture_uuid(0xfb);
const COMMAND: Uuid = fixture_uuid(0x10);
const OTHER_COMMAND: Uuid = fixture_uuid(0x11);
const ORIGIN: &str = "https://fixture.invalid";
const CODE: &str = "C1-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

const fn fixture_uuid(suffix: u128) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 + suffix)
}

fn time() -> Timestamp {
    Timestamp::parse_from_rfc3339("2026-11-21T00:00:00Z").expect("夹具时刻")
}

fn status(enabled: bool) -> DepositStatus {
    DepositStatus {
        enabled,
        registration_ready: enabled,
        registration_modes: if enabled {
            vec![
                auction_domain::consignment::ListingMode::Direct,
                auction_domain::consignment::ListingMode::Consigned,
            ]
        } else {
            Vec::new()
        },
        registration_worlds: if enabled {
            vec![SettlementWorld::Main, SettlementWorld::Isekai]
        } else {
            Vec::new()
        },
        fee_bps: FeeBps::DEFAULT_CONSIGNED,
        min_price: ListingPrice::new(ListingPrice::MIN).expect("最低价"),
        max_price: ListingPrice::new(ListingPrice::MAX).expect("最高价"),
        accepts_mail: true,
        purchasable: false,
        worlds: DepositWorlds {
            main: DepositWorldStatus {
                mail_ready: true,
                purchase_ready: false,
                season_state: SeasonState::NotSeasonal,
                cutoff_at: None,
                season_end_at: None,
            },
            isekai: DepositWorldStatus {
                mail_ready: true,
                purchase_ready: false,
                season_state: SeasonState::Open,
                cutoff_at: Some(time()),
                season_end_at: Some(time()),
            },
        },
    }
}

fn active() -> OwnedDepositInstructions {
    OwnedDepositInstructions::Active(ActiveDepositInstructions {
        world: SettlementWorld::Main,
        bot: "FixtureBot".to_owned(),
        subject: "consign".to_owned(),
        body: format_token([0; 32]),
        reservation_id: fixture_uuid(0x20),
        expires_at: time(),
    })
}

struct Backend {
    status: Mutex<Result<DepositStatus, ConsignmentError>>,
    instructions: Mutex<HashMap<Uuid, OwnedDepositInstructions>>,
    managed: Mutex<HashSet<Uuid>>,
    reservations: Mutex<HashMap<Uuid, (IssueReservationCommand, ReservationReceipt)>>,
    reservation_failure: Mutex<Option<ConsignmentError>>,
    reservation_listing_override: Mutex<Option<Uuid>>,
    reservation_state_override: Mutex<Option<ListingState>>,
    withdrawals: Mutex<HashMap<Uuid, (WithdrawDepositCommand, WithdrawDepositReceipt)>>,
    withdrawal_failure: Mutex<Option<ConsignmentError>>,
    v1_versions: Mutex<HashMap<Uuid, u64>>,
    deposit_calls: Mutex<Vec<Uuid>>,
    v1_calls: Mutex<Vec<Uuid>>,
}

impl Default for Backend {
    fn default() -> Self {
        let mut managed = HashSet::new();
        managed.insert(LISTING);
        let mut instructions = HashMap::new();
        instructions.insert(LISTING, active());
        Self {
            status: Mutex::new(Ok(status(true))),
            instructions: Mutex::new(instructions),
            managed: Mutex::new(managed),
            reservations: Mutex::new(HashMap::new()),
            reservation_failure: Mutex::new(None),
            reservation_listing_override: Mutex::new(None),
            reservation_state_override: Mutex::new(None),
            withdrawals: Mutex::new(HashMap::new()),
            withdrawal_failure: Mutex::new(None),
            v1_versions: Mutex::new(HashMap::from([(LISTING, 3), (UNMANAGED, 1)])),
            deposit_calls: Mutex::new(Vec::new()),
            v1_calls: Mutex::new(Vec::new()),
        }
    }
}

impl Backend {
    fn set_instructions(&self, value: OwnedDepositInstructions) {
        self.instructions.lock().expect("预约投影").insert(LISTING, value);
    }

    fn set_status(&self, value: Result<DepositStatus, ConsignmentError>) {
        *self.status.lock().expect("能力投影") = value;
    }

    fn submit_reservation(
        &self,
        command: IssueReservationCommand,
    ) -> Result<ReservationReceipt, ConsignmentError> {
        self.deposit_calls.lock().expect("寄售端口").push(command.listing_id);
        if let Some(error) = *self.reservation_failure.lock().expect("预约失败") {
            return Err(error);
        }
        let mut records = self.reservations.lock().expect("预约账本");
        if let Some((first, receipt)) = records.get(&command.command_id.as_uuid()) {
            return if first.request_hash == command.request_hash {
                Ok(receipt.clone())
            } else {
                Err(ConsignmentError::CommandIdConflict)
            };
        }
        if command.listing_id == OTHER_LISTING {
            return Err(ConsignmentError::Business(
                auction_application::consignment::ListingBusinessRejection::ListingNotFound,
            ));
        }
        let receipt_listing = self
            .reservation_listing_override
            .lock()
            .expect("预约回执挂单")
            .unwrap_or(command.listing_id);
        let receipt_state = self
            .reservation_state_override
            .lock()
            .expect("预约回执状态")
            .unwrap_or(ListingState::AwaitingDeposit);
        let receipt = ReservationReceipt::issued(
            command.command_id,
            receipt_listing,
            ListingVersion::new(command.expected_version.value() + 1).expect("版本"),
            receipt_state,
            fixture_uuid(0x30),
            time(),
        );
        records.insert(command.command_id.as_uuid(), (command, receipt.clone()));
        Ok(receipt)
    }

    fn submit_withdrawal(
        &self,
        command: WithdrawDepositCommand,
    ) -> Result<WithdrawDepositReceipt, ConsignmentError> {
        self.deposit_calls.lock().expect("寄售端口").push(command.listing_id);
        if let Some(error) = *self.withdrawal_failure.lock().expect("撤回失败") {
            return Err(error);
        }
        let mut records = self.withdrawals.lock().expect("撤回账本");
        if let Some((first, receipt)) = records.get(&command.command_id.as_uuid()) {
            return if first.request_hash == command.request_hash {
                Ok(receipt.clone())
            } else {
                Err(ConsignmentError::CommandIdConflict)
            };
        }
        if command.listing_id == OTHER_LISTING {
            return Err(ConsignmentError::Business(
                auction_application::consignment::ListingBusinessRejection::ListingNotFound,
            ));
        }
        let managed = self.managed.lock().expect("代管集合").contains(&command.listing_id);
        let receipt = if managed {
            WithdrawDepositReceipt::new(
                command.command_id,
                WithdrawDisposition::WithdrawalRequested,
                command.listing_id,
                ListingVersion::new(command.expected_version.value() + 1).expect("版本"),
                ListingState::WithdrawalPending,
                Some(fixture_uuid(0x40)),
            )
        } else {
            WithdrawDepositReceipt::new(
                command.command_id,
                WithdrawDisposition::Closed,
                command.listing_id,
                ListingVersion::new(command.expected_version.value() + 1).expect("版本"),
                ListingState::Closed,
                None,
            )
        };
        records.insert(command.command_id.as_uuid(), (command, receipt.clone()));
        Ok(receipt)
    }
}

impl DepositRead for Backend {
    fn status(&self) -> ConsignmentFuture<'_, DepositStatus> {
        let result = self.status.lock().expect("能力投影").clone();
        Box::pin(async move { result })
    }

    fn instructions(
        &self,
        seller: UserId,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<OwnedDepositInstructions>> {
        let value = if seller.as_uuid() != SELLER {
            None
        } else {
            self.instructions.lock().expect("预约投影").get(&listing_id).cloned()
        };
        Box::pin(async move { Ok(value) })
    }

    fn quarantine(
        &self,
        _query: AdminDepositQuery,
    ) -> ConsignmentFuture<'_, Option<AdminDepositDetail>> {
        Box::pin(async { Ok(None) })
    }
}

impl ReservationStore for Backend {
    fn issue_reservation(
        &self,
        command: IssueReservationCommand,
    ) -> ConsignmentFuture<'_, ReservationReceipt> {
        let result = self.submit_reservation(command);
        Box::pin(async move { result })
    }

    fn expire_reservation(
        &self,
        _command: auction_application::consignment::ExpireReservationCommand,
    ) -> ConsignmentFuture<'_, auction_application::consignment::ReservationRetired> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn replay_issue_reservation(
        &self,
        seller_user_id: UserId,
        command_id: CommandId,
        listing_id: Uuid,
        request_hash: RequestHash,
    ) -> ConsignmentFuture<'_, Option<ReservationReceipt>> {
        let records = self.reservations.lock().expect("预约账本");
        let result = match records.get(&command_id.as_uuid()) {
            None => Ok(None),
            Some((command, receipt))
                if command.seller_user_id == seller_user_id
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

impl CustodyControlStore for Backend {
    fn request_withdrawal(
        &self,
        command: WithdrawDepositCommand,
    ) -> ConsignmentFuture<'_, WithdrawDepositReceipt> {
        let result = self.submit_withdrawal(command);
        Box::pin(async move { result })
    }

    fn set_operations_hold(
        &self,
        _command: auction_application::consignment::SetOperationsHoldCommand,
    ) -> ConsignmentFuture<'_, auction_application::consignment::OperationsHoldReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn release_operations_hold(
        &self,
        _command: auction_application::consignment::ReleaseOperationsHoldCommand,
    ) -> ConsignmentFuture<'_, auction_application::consignment::OperationsHoldReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn replay_set_operations_hold(
        &self,
        _admin_actor: String,
        _command_id: CommandId,
        _listing_id: Uuid,
        _request_hash: RequestHash,
    ) -> ConsignmentFuture<'_, Option<auction_application::consignment::OperationsHoldReceipt>>
    {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }
}

impl EnvelopeStore for Backend {
    fn decide_envelope(
        &self,
        _command: DecideEnvelopeCommand,
    ) -> ConsignmentFuture<'_, EnvelopeReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn append_quarantine_observation(
        &self,
        _command: AppendQuarantineObservationCommand,
    ) -> ConsignmentFuture<'_, ObservationReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn recheck_quarantine(
        &self,
        _command: RecheckQuarantineCommand,
    ) -> ConsignmentFuture<'_, RecheckReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn release_quarantine(
        &self,
        _command: ReleaseQuarantineCommand,
    ) -> ConsignmentFuture<'_, ReleaseReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
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
        _replay_crypto: Option<&'a dyn RegistrationReplayCrypto>,
    ) -> ConsignmentFuture<'a, ListingCommandOutcome<ListingRegistered>> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn reprice(
        &self,
        command: RepriceListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingPriceReceipt>> {
        let managed = self.managed.lock().expect("代管集合").contains(&command.target.listing_id);
        if managed {
            return Box::pin(async {
                Err(ConsignmentError::from(
                    auction_domain::consignment::ListingError::InvalidTransition {
                        from: "available",
                        action: "reprice",
                    },
                ))
            });
        }
        let mut versions = self.v1_versions.lock().expect("v1 版本");
        let version = versions.entry(command.target.listing_id).or_insert(1);
        *version += 1;
        let receipt = ListingPriceReceipt::new(
            command.target.listing_id,
            ListingVersion::new(*version).expect("版本"),
            ListingState::Displayed,
            command.price,
            auction_application::consignment::ListingPriceDisposition::Repriced,
        );
        Box::pin(async move { Ok(ListingCommandOutcome::Accepted(receipt?)) })
    }

    fn withdraw(
        &self,
        command: WithdrawListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
        self.v1_calls.lock().expect("v1 端口").push(command.target.listing_id);
        let managed = self.managed.lock().expect("代管集合").contains(&command.target.listing_id);
        let next_version = (!managed).then(|| {
            let mut versions = self.v1_versions.lock().expect("v1 版本");
            let version = versions.entry(command.target.listing_id).or_insert(1);
            *version += 1;
            *version
        });
        Box::pin(async move {
            if managed {
                return Err(ConsignmentError::from(
                    auction_domain::consignment::ListingError::InvalidTransition {
                        from: "available",
                        action: "withdraw",
                    },
                ));
            }
            let next_version = next_version.expect("成功路径版本");
            let closed = ListingClosed::new(
                command.target.listing_id,
                ListingVersion::new(next_version).expect("版本"),
                Some(ListingPrice::new(101).expect("标价")),
            )?;
            Ok(ListingCommandOutcome::Accepted(closed))
        })
    }

    fn apply_lookup(
        &self,
        _command: ApplyListingLookupCommand,
    ) -> ConsignmentFuture<
        '_,
        ListingCommandOutcome<auction_application::consignment::ListingLookupEffect>,
    > {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn auto_close(
        &self,
        _command: AutoCloseListingCommand,
    ) -> ConsignmentFuture<
        '_,
        ListingCommandOutcome<auction_application::consignment::ListingAutoCloseEffect>,
    > {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn admin_delist(
        &self,
        _command: AdminDelistListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }
}

#[derive(Default)]
struct Sessions;

impl PublicSessionPort for Sessions {
    fn current(
        &self,
        token: String,
        _now: Timestamp,
    ) -> ApplicationFuture<'_, PublicSessionIdentity> {
        Box::pin(async move {
            let user_id = match token.as_str() {
                "seller" => SELLER,
                "other" => OTHER_SELLER,
                _ => return Err(SessionError::Unauthenticated.into()),
            };
            Ok(PublicSessionIdentity {
                user_id,
                username: "FixtureSeller".to_owned(),
                expires_at: None,
                absolute_expires_at: None,
            })
        })
    }

    fn inspect(
        &self,
        _token: String,
        _now: Timestamp,
    ) -> ApplicationFuture<'_, PublicSessionIdentity> {
        Box::pin(async { Err(SessionError::Unauthenticated.into()) })
    }

    fn login(&self, _key: Vec<u8>, _now: Timestamp) -> ApplicationFuture<'_, PublicLoginResult> {
        Box::pin(async { Err(SessionError::Unauthenticated.into()) })
    }

    fn logout(&self, _token: String, _now: Timestamp) -> ApplicationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn get_preference(
        &self,
        _token: String,
        _now: Timestamp,
    ) -> ApplicationFuture<'_, PublicSessionPreferenceResult> {
        Box::pin(async { Err(SessionError::Unauthenticated.into()) })
    }

    fn set_preference(
        &self,
        _token: String,
        _command: SetPublicSessionPreferenceCommand,
        _now: Timestamp,
    ) -> ApplicationFuture<'_, PublicSessionPreferenceResult> {
        Box::pin(async { Err(SessionError::Unauthenticated.into()) })
    }
}

#[derive(Clone)]
struct TestState {
    base: PublicApiState,
    deposit_caps: DepositCapabilities,
    v1: ConsignmentCapabilities,
}

impl ConsignmentState for TestState {
    fn public_state(&self) -> &PublicApiState {
        &self.base
    }

    fn consignment(&self) -> &ConsignmentCapabilities {
        &self.v1
    }
}

impl DepositState for TestState {
    fn deposit(&self) -> &DepositCapabilities {
        &self.deposit_caps
    }
}

fn setup(backend: &Arc<Backend>, enabled: bool) -> TestState {
    let security = PublicHttpSecurity::new(HttpSecurityOptions {
        cookie_secure: false,
        origins_enabled: true,
        allowed_origins: &[ORIGIN.to_owned()],
        login_limit: 20,
        mutation_limit: 100,
        anonymous_read_limit: 100,
        anonymous_read_concurrency: 4,
        rate_limit_window: Duration::from_secs(60),
        request_timeout: Duration::from_secs(5),
        trusted_proxy_cidrs: &[],
        trusted_unix_proxy: false,
    })
    .expect("安全策略");
    let session: Arc<dyn PublicSessionPort> = Arc::new(Sessions);
    let base = PublicApiState::new(
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
        Some(session),
        security,
    );
    let service = Arc::new(DepositCommandService::new(
        backend.clone(),
        backend.clone(),
        backend.clone(),
        backend.clone(),
        backend.clone(),
    ));
    TestState {
        base,
        deposit_caps: DepositCapabilities {
            enabled,
            read: Some(backend.clone()),
            commands: Some(service),
        },
        v1: ConsignmentCapabilities {
            manage: Some(Arc::new(ManageListingService::new(backend.clone(), true))),
            ..ConsignmentCapabilities::default()
        },
    }
}

fn router(state: TestState) -> Router {
    let routes = mount_v1(mount(Router::new()));
    routes.with_state(state).layer(middleware::from_fn_with_state(HttpBoundary::default(), enforce))
}

/// 只挂 v2 路由，不套全局边界；用于证明 `no-store` 来自这些路由自身的中间件。
fn v2_only_router(state: TestState) -> Router {
    mount(Router::new()).with_state(state)
}

fn request(method: Method, uri: &str, body: Body, user: Option<Uuid>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-request-id", "fixture-deposit")
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(user) = user {
        let cookie = if user == OTHER_SELLER {
            "auction_public_session=other"
        } else {
            "auction_public_session=seller"
        };
        builder = builder
            .header(header::COOKIE, cookie)
            .header(PUBLIC_EXPECTED_USER_ID_HEADER, user.to_string());
    }
    let mut request = builder.body(body).expect("HTTP 请求");
    request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 22], 3123))));
    request
}

fn get(uri: &str, user: Option<Uuid>) -> Request<Body> {
    request(Method::GET, uri, Body::empty(), user)
}

fn post(uri: &str, body: Value) -> Request<Body> {
    request(Method::POST, uri, Body::from(body.to_string()), Some(SELLER))
}

struct Wire {
    status: u16,
    headers: HeaderMap,
    body: Value,
}

impl Wire {
    fn assert_code(&self, status: u16, code: &str) {
        assert_eq!(self.status, status, "响应正文 {}", self.body);
        assert_eq!(self.body["code"], code);
    }

    fn assert_no_secret(&self) {
        let wire = self.body.to_string();
        assert!(!wire.contains(CODE), "响应正文泄露预约码");
        for value in self.headers.values() {
            assert!(!String::from_utf8_lossy(value.as_bytes()).contains(CODE), "响应头泄露预约码");
        }
    }
}

async fn send(app: &Router, request: Request<Body>) -> Wire {
    let (parts, body) = app.clone().oneshot(request).await.expect("HTTP 响应").into_parts();
    assert_eq!(parts.headers[header::CACHE_CONTROL], "no-store");
    assert!(!parts.headers.contains_key(header::ETAG));
    for value in parts.headers.values() {
        assert!(!String::from_utf8_lossy(value.as_bytes()).contains(CODE));
    }
    let bytes = to_bytes(body, 2 << 20).await.expect("HTTP 正文");
    Wire {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body: serde_json::from_slice(&bytes).expect("HTTP JSON"),
    }
}

#[tokio::test]
async fn status_keeps_disabled_as_200_and_reports_both_worlds() {
    let backend = Arc::new(Backend::default());
    backend.set_status(Ok(status(false)));
    let app = router(setup(&backend, false));
    let wire = send(&app, get(STATUS, None)).await;
    wire.assert_code(200, "ok");
    assert_eq!(wire.body["data"]["enabled"], false);
    assert_eq!(wire.body["data"]["worlds"]["main"]["season_state"], "not_seasonal");
    assert_eq!(wire.body["data"]["worlds"]["isekai"]["season_state"], "open");
    assert_eq!(wire.body["data"]["accepts_mail"], true);
    assert_eq!(wire.body["data"]["purchasable"], false);
    wire.assert_no_secret();
}

#[tokio::test]
async fn instructions_are_owner_only_and_none_never_contains_the_code() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, true));
    let active =
        send(&app, get(&format!("{OWNED}/{LISTING}/deposit-instructions"), Some(SELLER))).await;
    active.assert_code(200, "ok");
    assert_eq!(active.body["data"]["state"], "active");
    assert_eq!(active.body["data"]["body"], CODE);
    assert_eq!(active.body["data"]["subject"], "consign");

    let other =
        send(&app, get(&format!("{OWNED}/{LISTING}/deposit-instructions"), Some(OTHER_SELLER)))
            .await;
    other.assert_code(404, "listing_not_found");
    other.assert_no_secret();

    backend.set_instructions(OwnedDepositInstructions::None);
    let none =
        send(&app, get(&format!("{OWNED}/{LISTING}/deposit-instructions"), Some(SELLER))).await;
    none.assert_code(200, "ok");
    assert_eq!(none.body["data"], json!({"state": "none"}));
    none.assert_no_secret();

    let disabled = router(setup(&backend, false));
    let disabled_instructions =
        send(&disabled, get(&format!("{OWNED}/{LISTING}/deposit-instructions"), Some(SELLER)))
            .await;
    disabled_instructions.assert_code(404, "consignment_disabled");
    disabled_instructions.assert_no_secret();
}

#[tokio::test]
async fn issue_reservation_replays_the_first_receipt_and_rejects_a_different_request() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, true));
    let body = json!({"command_id": COMMAND, "expected_version": 3});
    let first =
        send(&app, post(&format!("{OWNED}/{LISTING}/deposit-reservations"), body.clone())).await;
    first.assert_code(201, "created");
    assert_eq!(first.body["data"]["operation"], "issue_reservation");
    assert_eq!(first.body["data"]["disposition"], "reservation_issued");
    assert_eq!(first.body["data"]["listing_id"], LISTING.to_string());
    assert_eq!(first.body["data"]["state"], "awaiting_deposit");

    let replay = send(&app, post(&format!("{OWNED}/{LISTING}/deposit-reservations"), body)).await;
    replay.assert_code(201, "created");
    assert_eq!(replay.body["data"], first.body["data"]);

    let conflict = send(
        &app,
        post(
            &format!("{OWNED}/{LISTING}/deposit-reservations"),
            json!({"command_id": COMMAND, "expected_version": 4}),
        ),
    )
    .await;
    conflict.assert_code(409, "command_id_conflict");
    conflict.assert_no_secret();
}

#[tokio::test]
async fn issue_reservation_replays_the_first_receipt_after_soft_close() {
    let backend = Arc::new(Backend::default());
    let enabled = router(setup(&backend, true));
    let body = json!({"command_id": COMMAND, "expected_version": 3});
    let first =
        send(&enabled, post(&format!("{OWNED}/{LISTING}/deposit-reservations"), body.clone()))
            .await;
    first.assert_code(201, "created");
    let calls = backend.deposit_calls.lock().expect("寄售端口").len();

    // 不变量 1、D9 §6.3：软关闭只截止新承诺；同一命令重放仍返回首次回执。
    let disabled = router(setup(&backend, false));
    let replay =
        send(&disabled, post(&format!("{OWNED}/{LISTING}/deposit-reservations"), body)).await;
    replay.assert_code(201, "created");
    assert_eq!(replay.body["data"], first.body["data"]);
    replay.assert_no_secret();

    let conflict = send(
        &disabled,
        post(
            &format!("{OWNED}/{LISTING}/deposit-reservations"),
            json!({"command_id": COMMAND, "expected_version": 4}),
        ),
    )
    .await;
    conflict.assert_code(409, "command_id_conflict");

    let fresh = send(
        &disabled,
        post(
            &format!("{OWNED}/{LISTING}/deposit-reservations"),
            json!({"command_id": OTHER_COMMAND, "expected_version": 3}),
        ),
    )
    .await;
    fresh.assert_code(404, "consignment_disabled");
    fresh.assert_no_secret();
    assert_eq!(
        backend.deposit_calls.lock().expect("寄售端口").len(),
        calls,
        "关闭后的重放与新 ID 都不得调用写端口"
    );
}

#[tokio::test]
async fn issue_reservation_keeps_trusted_and_unknown_failures_distinct() {
    let backend = Arc::new(Backend::default());
    *backend.reservation_failure.lock().expect("预约失败") = Some(ConsignmentError::Deposit(
        auction_application::consignment::DepositBusinessRejection::MailUnavailable,
    ));
    let app = router(setup(&backend, true));
    let body = json!({"command_id": COMMAND, "expected_version": 3});
    let mail =
        send(&app, post(&format!("{OWNED}/{LISTING}/deposit-reservations"), body.clone())).await;
    mail.assert_code(503, "mail_unavailable");
    mail.assert_no_secret();

    *backend.reservation_failure.lock().expect("预约失败") =
        Some(ConsignmentError::Business(ListingBusinessRejection::ListingLimitReached));
    let limited =
        send(&app, post(&format!("{OWNED}/{LISTING}/deposit-reservations"), body.clone())).await;
    limited.assert_code(429, "listing_limit_reached");
    limited.assert_no_secret();

    *backend.reservation_failure.lock().expect("预约失败") = Some(ConsignmentError::OutcomeUnknown);
    let unknown = send(&app, post(&format!("{OWNED}/{LISTING}/deposit-reservations"), body)).await;
    unknown.assert_code(503, "outcome_unknown");
    unknown.assert_no_secret();

    let disabled = router(setup(&backend, false));
    let disabled = send(
        &disabled,
        post(
            &format!("{OWNED}/{LISTING}/deposit-reservations"),
            json!({"command_id": COMMAND, "expected_version": 3}),
        ),
    )
    .await;
    disabled.assert_code(404, "consignment_disabled");
    disabled.assert_no_secret();
}

#[tokio::test]
async fn issue_reservation_rejects_a_receipt_for_another_listing_as_outcome_unknown() {
    let backend = Arc::new(Backend::default());
    *backend.reservation_listing_override.lock().expect("预约回执挂单") = Some(OTHER_LISTING);
    let app = router(setup(&backend, true));
    let response = send(
        &app,
        post(
            &format!("{OWNED}/{LISTING}/deposit-reservations"),
            json!({"command_id": COMMAND, "expected_version": 3}),
        ),
    )
    .await;
    response.assert_code(503, "outcome_unknown");
    response.assert_no_secret();
}

#[tokio::test]
async fn issue_reservation_rejects_a_receipt_with_a_non_awaiting_state_as_outcome_unknown() {
    let backend = Arc::new(Backend::default());
    *backend.reservation_state_override.lock().expect("预约回执状态") =
        Some(ListingState::Available);
    let app = router(setup(&backend, true));
    let response = send(
        &app,
        post(
            &format!("{OWNED}/{LISTING}/deposit-reservations"),
            json!({"command_id": COMMAND, "expected_version": 3}),
        ),
    )
    .await;
    response.assert_code(503, "outcome_unknown");
    response.assert_no_secret();
}

#[tokio::test]
async fn withdraw_managed_is_202_and_unmanaged_closes_with_200() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, true));
    let managed = send(
        &app,
        post(
            &format!("{WRITE}/{LISTING}/withdraw"),
            json!({"command_id": COMMAND, "expected_version": 3}),
        ),
    )
    .await;
    managed.assert_code(202, "accepted");
    assert_eq!(managed.body["data"]["disposition"], "withdrawal_requested");
    assert_eq!(managed.body["data"]["state"], "withdrawal_pending");

    let unmanaged = send(
        &app,
        post(
            &format!("{WRITE}/{UNMANAGED}/withdraw"),
            json!({"command_id": OTHER_COMMAND, "expected_version": 3}),
        ),
    )
    .await;
    unmanaged.assert_code(200, "ok");
    assert_eq!(unmanaged.body["data"]["operation"], "withdraw");
    assert_eq!(unmanaged.body["data"]["disposition"], "closed");
    assert_eq!(unmanaged.body["data"]["state"], "closed");
    assert_eq!(unmanaged.body["data"]["listing_id"], UNMANAGED.to_string());
    assert!(unmanaged.body["data"].get("intent_id").is_none());
    assert_eq!(backend.deposit_calls.lock().expect("寄售端口").as_slice(), &[LISTING, UNMANAGED]);
    unmanaged.assert_no_secret();
}

#[tokio::test]
async fn public_v2_routes_require_session_csrf_origin_and_strict_methods() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, true));
    let unauthorized = send(
        &app,
        request(
            Method::POST,
            &format!("{OWNED}/{LISTING}/deposit-reservations"),
            Body::from(json!({"command_id": COMMAND, "expected_version": 3}).to_string()),
            None,
        ),
    )
    .await;
    unauthorized.assert_code(401, "unauthenticated");
    assert!(backend.deposit_calls.lock().expect("寄售端口").is_empty());

    let mut bad_origin = post(
        &format!("{OWNED}/{LISTING}/deposit-reservations"),
        json!({"command_id": COMMAND, "expected_version": 3}),
    );
    bad_origin
        .headers_mut()
        .insert(header::ORIGIN, HeaderValue::from_static("https://other.invalid"));
    let forbidden = send(&app, bad_origin).await;
    forbidden.assert_code(403, "origin_forbidden");
    assert!(backend.deposit_calls.lock().expect("寄售端口").is_empty());

    let wrong_method =
        send(&app, get(&format!("{OWNED}/{LISTING}/deposit-reservations"), Some(SELLER))).await;
    assert_eq!(wrong_method.status, 405);
    wrong_method.assert_no_secret();

    let query = send(
        &app,
        get(&format!("{OWNED}/{LISTING}/deposit-instructions?unexpected=1"), Some(SELLER)),
    )
    .await;
    query.assert_code(400, "invalid_request");
    query.assert_no_secret();
}

#[tokio::test]
async fn v1_status_stays_closed_and_managed_v1_mutations_are_listing_not_active() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, true));
    let v1_status = send(&app, get("/api/v1/consignment/status", None)).await;
    v1_status.assert_code(200, "ok");
    assert_eq!(v1_status.body["data"]["accepts_mail"], false);
    assert_eq!(v1_status.body["data"]["purchasable"], false);

    let v1_withdraw = send(
        &app,
        post(
            &format!("/api/v1/consignment/listings/{LISTING}/withdraw"),
            json!({"command_id": COMMAND, "expected_version": 3}),
        ),
    )
    .await;
    v1_withdraw.assert_code(409, "listing_not_active");
    assert_eq!(backend.v1_versions.lock().expect("v1 版本")[&LISTING], 3);

    let v1_reprice = send(
        &app,
        request(
            Method::PATCH,
            &format!("/api/v1/consignment/listings/{LISTING}/price"),
            Body::from(
                json!({"command_id": OTHER_COMMAND, "expected_version": 3, "price": 120})
                    .to_string(),
            ),
            Some(SELLER),
        ),
    )
    .await;
    v1_reprice.assert_code(409, "listing_not_active");
    assert_eq!(backend.v1_versions.lock().expect("v1 版本")[&LISTING], 3);

    let v1_unmanaged = send(
        &app,
        post(
            &format!("/api/v1/consignment/listings/{UNMANAGED}/withdraw"),
            json!({"command_id": fixture_uuid(0x12), "expected_version": 1}),
        ),
    )
    .await;
    v1_unmanaged.assert_code(200, "ok");
    assert_eq!(v1_unmanaged.body["data"]["disposition"], "closed");
    assert_eq!(backend.v1_versions.lock().expect("v1 版本")[&UNMANAGED], 2);
    assert_eq!(backend.v1_calls.lock().expect("v1 端口").as_slice(), &[LISTING, UNMANAGED]);
    assert!(backend.deposit_calls.lock().expect("寄售端口").is_empty());
}

#[tokio::test]
async fn all_v2_responses_keep_no_store_including_errors_and_404() {
    let backend = Arc::new(Backend::default());
    let app = router(setup(&backend, true));
    for reply in [
        send(&app, get(&format!("{OWNED}/{OTHER_LISTING}/deposit-instructions"), Some(SELLER)))
            .await,
        send(&app, get(&format!("{OWNED}/not-a-uuid/deposit-instructions"), Some(SELLER))).await,
        send(
            &app,
            post(
                &format!("{OWNED}/{LISTING}/deposit-reservations"),
                json!({"command_id": "not-a-uuid", "expected_version": 3}),
            ),
        )
        .await,
    ] {
        assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");
        reply.assert_no_secret();
    }
}

#[tokio::test]
async fn v2_success_responses_get_no_store_from_the_route_layer() {
    let backend = Arc::new(Backend::default());
    let app = v2_only_router(setup(&backend, true));
    let status = send(&app, get(STATUS, None)).await;
    status.assert_code(200, "ok");
    let instructions =
        send(&app, get(&format!("{OWNED}/{LISTING}/deposit-instructions"), Some(SELLER))).await;
    instructions.assert_code(200, "ok");
    let reservation = send(
        &app,
        post(
            &format!("{OWNED}/{LISTING}/deposit-reservations"),
            json!({"command_id": COMMAND, "expected_version": 3}),
        ),
    )
    .await;
    reservation.assert_code(201, "created");
    let withdraw = send(
        &app,
        post(
            &format!("{WRITE}/{UNMANAGED}/withdraw"),
            json!({"command_id": OTHER_COMMAND, "expected_version": 3}),
        ),
    )
    .await;
    withdraw.assert_code(200, "ok");
}
