//! 处理器测试用的会话、寄售端口和离线请求夹具。

use std::future::pending;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::admin::{AdminIntakeError, EquipmentKeySealerPort};
use auction_application::consignment::{
    AdminDelistListingCommand, ApplyListingLookupCommand, AutoCloseListingCommand,
    ConsignmentError, ConsignmentFuture, ListingAutoCloseEffect, ListingBusinessRejection,
    ListingClosed, ListingCommandOutcome, ListingCommandRejection, ListingCommandStore,
    ListingCredentialError, ListingEquipmentReference, ListingEquipmentUrlValidator,
    ListingLookupEffect, ListingPageWindow, ListingPriceDisposition, ListingPriceReceipt,
    ListingReadPage, ListingRegistered, ListingRegistrationAvailability, ListingRegistrationPolicy,
    ListingWorlds, ManageListingService, OwnedListingCommandReceipt, OwnedListingRead,
    OwnedListingRecord, PrivateListingQuery, PublicListingDetail, PublicListingQuery,
    PublicListingRead, PublicListingSummary, RegisterListingCommand, RegisterListingService,
    RegistrationInput, RegistrationReplayCrypto, RegistrationRequestMac, RepriceListingCommand,
    StoredListingDecision, UnrecordedListingRejection, WithdrawListingCommand,
    WrappedRegistrationReplayKey,
};
use auction_application::core::{ApplicationFuture, SessionError};
use auction_application::public::{
    PublicLoginResult, PublicSessionIdentity, PublicSessionPort, PublicSessionPreferenceResult,
    SetPublicSessionPreferenceCommand,
};
use auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER;
use auction_domain::consignment::{
    FeeBps, ListingEquipmentIdentity, ListingMode, ListingPrice, ListingState, ListingVersion,
};
use auction_domain::{CommandId, EquipmentId, EquipmentKey, EquipmentKeyEnvelope, UserId};
use auction_time::Timestamp;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, HeaderValue, Method, Request, header};
use axum::{Router, middleware};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

use crate::middleware::boundary::{HttpBoundary, enforce};
use crate::middleware::security::{HttpSecurityOptions, PublicHttpSecurity};
use crate::public::consignment::{ConsignmentCapabilities, ConsignmentState, mount};
use crate::state::PublicApiState;

pub(super) const LISTINGS: &str = "/api/v1/consignment/listings";
pub(super) const OWNED: &str = "/api/v1/me/consignment/listings";
pub(super) const COMMANDS: &str = "/api/v1/me/consignment/commands";
pub(super) const STATUS: &str = "/api/v1/consignment/status";
pub(super) const LISTING: Uuid = fixture_uuid(1);
pub(super) const SELLER: Uuid = fixture_uuid(0xfa);
pub(super) const OTHER_SELLER: Uuid = fixture_uuid(0xfb);
pub(super) const FAKE_KEY: &str = "deadbeef00";
pub(super) const FAKE_LINK: &str = "https://fixture.invalid/equip/42/deadbeef00";
pub(super) const GOOD_ORIGIN: &str = "https://fixture.invalid";
pub(super) const EXPIRED_COOKIE: &str = "auction_public_session=fixture-expired";
const GOOD_COOKIE: &str = "auction_public_session=fixture-seller";
const REQUEST_ID: &str = "consignment-fixture";

pub(super) const fn fixture_uuid(suffix: u128) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 + suffix)
}

pub(super) fn price(amount: i64) -> ListingPrice {
    ListingPrice::new(amount).expect("夹具标价")
}

pub(super) fn version(number: u64) -> ListingVersion {
    ListingVersion::new(number).expect("夹具版本")
}

pub(super) fn time(raw: &str) -> Timestamp {
    Timestamp::parse_from_rfc3339(raw).expect("夹具时刻")
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Call {
    Availability,
    PublicList(PublicListingQuery),
    PublicDetail(Uuid),
    OwnedList(UserId, PrivateListingQuery),
    OwnedDetail(UserId, Uuid),
    OwnedCommand(UserId, CommandId),
    ValidateUrl,
    Register(UserId, CommandId, ListingMode, i64, u16, bool, u16),
    Reprice(UserId, CommandId, Uuid, u64, i64, bool),
    Withdraw(UserId, CommandId, Uuid, u64),
}

#[derive(Clone, Copy, Default)]
pub(super) enum WriteOutcome {
    #[default]
    Accepted,
    Replay,
    Reject(ListingBusinessRejection),
    ReplayRejected(ListingBusinessRejection),
    Conflict,
    Err(ConsignmentError),
    Unchanged,
    CloseWithoutPrice,
    WrongTarget,
}

pub(super) struct Backend {
    pub(super) calls: Mutex<Vec<Call>>,
    pub(super) request_ids: Mutex<Vec<Option<String>>>,
    pub(super) availability: Result<ListingRegistrationAvailability, ConsignmentError>,
    pub(super) availability_stalls: bool,
    pub(super) public: Option<PublicListingDetail>,
    pub(super) owned: Option<OwnedListingRecord>,
    pub(super) command: Option<OwnedListingCommandReceipt>,
    pub(super) return_wrong_command_id: bool,
    pub(super) read_error: Option<ConsignmentError>,
    pub(super) command_outcome: Mutex<WriteOutcome>,
    pub(super) page_total: Option<u64>,
    pub(super) window_override: Option<ListingPageWindow>,
}

impl Default for Backend {
    fn default() -> Self {
        Self {
            calls: Mutex::default(),
            request_ids: Mutex::default(),
            availability: Ok(ListingRegistrationAvailability::Ready(ListingWorlds {
                main: true,
                isekai: true,
            })),
            availability_stalls: false,
            public: None,
            owned: None,
            command: None,
            return_wrong_command_id: false,
            read_error: None,
            command_outcome: Mutex::default(),
            page_total: None,
            window_override: None,
        }
    }
}

impl Backend {
    pub(super) fn take_calls(&self) -> Vec<Call> {
        std::mem::take(&mut *self.calls.lock().expect("端口调用"))
    }

    pub(super) fn take_request_ids(&self) -> Vec<Option<String>> {
        std::mem::take(&mut *self.request_ids.lock().expect("命令请求号"))
    }

    fn record(&self, call: Call) {
        self.calls.lock().expect("端口调用").push(call);
    }

    fn read<T: Send + 'static>(&self, result: T) -> ConsignmentFuture<'_, T> {
        let error = self.read_error;
        Box::pin(async move { error.map_or(Ok(result), Err) })
    }

    fn page<T>(&self, window: ListingPageWindow, record: Option<T>) -> ListingReadPage<T> {
        ListingReadPage {
            total: self.page_total.unwrap_or_else(|| u64::from(record.is_some())),
            items: record.into_iter().collect(),
            window: self.window_override.unwrap_or(window),
        }
    }

    fn outcome(&self) -> WriteOutcome {
        *self.command_outcome.lock().expect("命令结果")
    }

    fn decide<T: Send + 'static>(
        &self,
        receipt: Result<T, ConsignmentError>,
        first_rejection: Option<UnrecordedListingRejection>,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<T>> {
        let outcome = self.outcome();
        Box::pin(async move {
            match outcome {
                WriteOutcome::Replay => {
                    Ok(ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(receipt?)))
                }
                WriteOutcome::ReplayRejected(error) => {
                    Ok(ListingCommandOutcome::Replayed(StoredListingDecision::Rejected(error)))
                }
                WriteOutcome::Conflict => {
                    Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Unrecorded(
                        UnrecordedListingRejection::CommandIdConflict,
                    )))
                }
                WriteOutcome::Err(error) => Err(error),
                WriteOutcome::Reject(error) => {
                    Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Recorded(error)))
                }
                _ => match first_rejection {
                    Some(error) => Ok(ListingCommandOutcome::Rejected(
                        ListingCommandRejection::Unrecorded(error),
                    )),
                    None => Ok(ListingCommandOutcome::Accepted(receipt?)),
                },
            }
        })
    }
}

impl PublicListingRead for Backend {
    fn list_public(
        &self,
        query: PublicListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<PublicListingSummary>> {
        let window = query.window;
        self.record(Call::PublicList(query));
        self.read(self.page(window, self.public.as_ref().map(|row| row.summary.clone())))
    }

    fn public_detail(
        &self,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<PublicListingDetail>> {
        self.record(Call::PublicDetail(listing_id));
        self.read(self.public.clone())
    }

    fn anonymous_command(
        &self,

        _command_id: CommandId,
    ) -> ConsignmentFuture<'_, Option<auction_application::consignment::AnonymousRegistrationReceipt>>
    {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }

    fn registration_availability<'a>(
        &'a self,
        _crypto: &'a dyn RegistrationReplayCrypto,
    ) -> ConsignmentFuture<'a, ListingRegistrationAvailability> {
        self.record(Call::Availability);
        if self.availability_stalls {
            Box::pin(pending())
        } else {
            Box::pin(async { self.availability })
        }
    }
}

impl OwnedListingRead for Backend {
    fn list_owned(
        &self,
        seller: UserId,
        query: PrivateListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<OwnedListingRecord>> {
        self.record(Call::OwnedList(seller, query));
        let row = self
            .owned
            .as_ref()
            .filter(|row| {
                seller.as_uuid() == SELLER
                    && query
                        .state
                        .is_none_or(|state| state == row.listing().view.effective_state().0)
            })
            .cloned();
        self.read(self.page(query.window, row))
    }

    fn owned_detail(
        &self,
        seller: UserId,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<OwnedListingRecord>> {
        self.record(Call::OwnedDetail(seller, listing_id));
        self.read(
            self.owned
                .as_ref()
                .filter(|row| seller.as_uuid() == SELLER && row.listing().listing_id == listing_id)
                .cloned(),
        )
    }

    fn owned_command(
        &self,
        seller: UserId,
        command_id: CommandId,
    ) -> ConsignmentFuture<'_, Option<OwnedListingCommandReceipt>> {
        self.record(Call::OwnedCommand(seller, command_id));
        self.read(self.command.filter(|record| {
            seller.as_uuid() == SELLER
                && (record.command_id == command_id || self.return_wrong_command_id)
        }))
    }
}

impl ListingCommandStore for Backend {
    fn register<'a>(
        &'a self,
        command: RegisterListingCommand<'a>,
        _sealer: Option<&'a dyn EquipmentKeySealerPort>,
        _crypto: Option<&'a dyn RegistrationReplayCrypto>,
    ) -> ConsignmentFuture<'a, ListingCommandOutcome<ListingRegistered>> {
        self.record(Call::Register(
            match command.input.seller {
                auction_application::consignment::RegistrationSeller::Account(seller) => seller,
                auction_application::consignment::RegistrationSeller::GameName(_) => {
                    panic!("账户登记夹具不接收匿名登记")
                }
            },
            command.command_id,
            command.input.mode,
            command.input.price.value(),
            command.input.expected_fee_bps.value(),
            command.policy.enabled,
            command.policy.consigned_fee_bps.value(),
        ));
        self.request_ids.lock().expect("命令请求号").push(command.request_id);
        if matches!(self.outcome(), WriteOutcome::Accepted)
            && command.policy.enabled
            && command.input.expected_fee_bps
                != FeeBps::for_mode(command.input.mode, command.policy.consigned_fee_bps)
        {
            return Box::pin(async {
                Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Recorded(
                    ListingBusinessRejection::FeeChanged,
                )))
            });
        }
        self.decide(
            ListingRegistered::new(
                if matches!(self.outcome(), WriteOutcome::Replay) {
                    LISTING
                } else {
                    command.listing_id
                },
                command.input.price,
            ),
            (!command.policy.enabled).then_some(UnrecordedListingRejection::Disabled),
        )
    }

    fn reprice(
        &self,
        command: RepriceListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingPriceReceipt>> {
        let target = command.target;
        self.record(Call::Reprice(
            target.seller_user_id,
            target.command_id,
            target.listing_id,
            target.expected_version.value(),
            command.price.value(),
            command.enabled,
        ));
        self.request_ids.lock().expect("命令请求号").push(command.request_id);
        let outcome = self.outcome();
        let (version, disposition) = if matches!(outcome, WriteOutcome::Unchanged) {
            (target.expected_version, ListingPriceDisposition::Unchanged)
        } else {
            (version(2), ListingPriceDisposition::Repriced)
        };
        let listing_id = if matches!(outcome, WriteOutcome::WrongTarget) {
            fixture_uuid(99)
        } else {
            target.listing_id
        };
        self.decide(
            ListingPriceReceipt::new(
                listing_id,
                version,
                ListingState::Resolving,
                command.price,
                disposition,
            ),
            (!command.enabled).then_some(UnrecordedListingRejection::Disabled),
        )
    }

    fn withdraw(
        &self,
        command: WithdrawListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
        let target = command.target;
        self.record(Call::Withdraw(
            target.seller_user_id,
            target.command_id,
            target.listing_id,
            target.expected_version.value(),
        ));
        self.request_ids.lock().expect("命令请求号").push(command.request_id);
        let outcome = self.outcome();
        let price = (!matches!(outcome, WriteOutcome::CloseWithoutPrice)).then(|| price(102));
        let listing_id = if matches!(outcome, WriteOutcome::WrongTarget) {
            fixture_uuid(99)
        } else {
            target.listing_id
        };
        self.decide(ListingClosed::new(listing_id, version(3), price), None)
    }

    fn apply_lookup(
        &self,
        _command: ApplyListingLookupCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingLookupEffect>> {
        panic!("Public 未装配解析命令")
    }

    fn auto_close(
        &self,
        _command: AutoCloseListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingAutoCloseEffect>> {
        panic!("Public 未装配补扫命令")
    }

    fn admin_delist(
        &self,
        _command: AdminDelistListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
        panic!("Public 未装配运营命令")
    }
}

impl ListingEquipmentUrlValidator for Backend {
    fn validate(&self, url: &str) -> Result<ListingEquipmentReference, ConsignmentError> {
        self.record(Call::ValidateUrl);
        if url != FAKE_LINK {
            return Err(ConsignmentError::InvalidEquipmentUrl);
        }
        Ok(ListingEquipmentReference {
            identity: ListingEquipmentIdentity::new("main", "42")?,
            key: EquipmentKey::new(FAKE_KEY).expect("假读取凭证格式"),
        })
    }
}

impl EquipmentKeySealerPort for Backend {
    fn seal(
        &self,
        _equipment_id: EquipmentId,
        _key: &EquipmentKey,
    ) -> Result<EquipmentKeyEnvelope, AdminIntakeError> {
        panic!("HTTP 不能封装装备读取凭证")
    }
}

impl RegistrationReplayCrypto for Backend {
    fn generate_wrapped(
        &self,
        _replay_key_id: Uuid,
    ) -> Result<WrappedRegistrationReplayKey, ListingCredentialError> {
        panic!("HTTP 不能生成比较钥")
    }

    fn check_wrapped(
        &self,
        _replay_key_id: Uuid,
        _wrapped: &WrappedRegistrationReplayKey,
    ) -> Result<(), ListingCredentialError> {
        panic!("HTTP 不能检查比较钥")
    }

    fn registration_mac(
        &self,
        _replay_key_id: Uuid,
        _wrapped: &WrappedRegistrationReplayKey,
        _input: &RegistrationInput<'_>,
    ) -> Result<RegistrationRequestMac, ListingCredentialError> {
        panic!("HTTP 不能计算命令摘要")
    }

    fn rewrap(
        &self,
        _replay_key_id: Uuid,
        _wrapped: &WrappedRegistrationReplayKey,
    ) -> Result<WrappedRegistrationReplayKey, ListingCredentialError> {
        panic!("HTTP 不能重封比较钥")
    }
}

#[derive(Default)]
pub(super) struct Session(Mutex<u32>);

impl Session {
    pub(super) fn take_calls(&self) -> u32 {
        std::mem::take(&mut *self.0.lock().expect("会话调用"))
    }
}

impl PublicSessionPort for Session {
    fn current(
        &self,
        token: String,
        _now: Timestamp,
    ) -> ApplicationFuture<'_, PublicSessionIdentity> {
        *self.0.lock().expect("会话调用") += 1;
        Box::pin(async move {
            let user_id = match token.as_str() {
                "fixture-seller" => SELLER,
                "fixture-other" => OTHER_SELLER,
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
        panic!("测试只读取当前会话")
    }

    fn login(&self, _key: Vec<u8>, _now: Timestamp) -> ApplicationFuture<'_, PublicLoginResult> {
        panic!("测试不登录")
    }

    fn logout(&self, _token: String, _now: Timestamp) -> ApplicationFuture<'_, ()> {
        panic!("测试不退出")
    }

    fn get_preference(
        &self,
        _token: String,
        _now: Timestamp,
    ) -> ApplicationFuture<'_, PublicSessionPreferenceResult> {
        panic!("测试不读会话偏好")
    }

    fn set_preference(
        &self,
        _token: String,
        _command: SetPublicSessionPreferenceCommand,
        _now: Timestamp,
    ) -> ApplicationFuture<'_, PublicSessionPreferenceResult> {
        panic!("测试不写会话偏好")
    }
}

#[derive(Clone)]
pub(super) struct TestState {
    base: PublicApiState,
    pub(super) capabilities: ConsignmentCapabilities,
}

impl ConsignmentState for TestState {
    fn public_state(&self) -> &PublicApiState {
        &self.base
    }
    fn consignment(&self) -> &ConsignmentCapabilities {
        &self.capabilities
    }
}

pub(super) fn setup(enabled: bool, backend: &Arc<Backend>) -> TestState {
    configured(enabled, backend, Some(Arc::new(Session::default())), 100)
}

pub(super) fn configured(
    enabled: bool,
    backend: &Arc<Backend>,
    session: Option<Arc<Session>>,
    mutation_limit: u32,
) -> TestState {
    let security = PublicHttpSecurity::new(HttpSecurityOptions {
        cookie_secure: false,
        origins_enabled: true,
        allowed_origins: &[GOOD_ORIGIN.to_owned()],
        login_limit: 20,
        mutation_limit,
        anonymous_read_limit: 100,
        anonymous_read_concurrency: 4,
        rate_limit_window: Duration::from_secs(60),
        request_timeout: Duration::from_secs(5),
        trusted_proxy_cidrs: &[],
        trusted_unix_proxy: false,
    })
    .expect("安全策略");
    let session = session.map(|session| session as Arc<dyn PublicSessionPort>);
    let base = PublicApiState::new(
        None, None, None, None, None, None, None, None, None, None, None, None, None, session,
        security,
    );
    let fee = FeeBps::DEFAULT_CONSIGNED;
    TestState {
        base,
        capabilities: ConsignmentCapabilities {
            enabled,
            fee_bps: fee,
            public_read: Some(backend.clone()),
            owned_read: Some(backend.clone()),
            register: Some(register_service(backend, enabled, fee)),
            manage: Some(Arc::new(ManageListingService::new(backend.clone(), enabled))),
            registration_sealer_available: true,
            replay_crypto: Some(backend.clone()),
        },
    }
}

pub(super) fn register_service(
    backend: &Arc<Backend>,
    enabled: bool,
    fee: FeeBps,
) -> Arc<RegisterListingService> {
    let next_uuid = std::sync::atomic::AtomicU64::new(1);
    Arc::new(RegisterListingService::new(
        backend.clone(),
        backend.clone(),
        Some(backend.clone()),
        Some(backend.clone()),
        ListingRegistrationPolicy {
            enabled,
            worlds: ListingWorlds { main: true, isekai: true },
            consigned_fee_bps: fee,
            max_open_per_user: 20,
            registrations_per_hour: 10,
            anonymous_resolving_limit: 50,
        },
        true,
        move || {
            fixture_uuid(u128::from(next_uuid.fetch_add(1, std::sync::atomic::Ordering::Relaxed)))
        },
    ))
}

pub(super) fn router(state: TestState) -> Router {
    router_with_boundary(state, HttpBoundary::default())
}

pub(super) fn router_with_boundary(state: TestState, boundary: HttpBoundary) -> Router {
    mount(Router::new()).with_state(state).layer(middleware::from_fn_with_state(boundary, enforce))
}

pub(super) fn get(uri: &str) -> Request<Body> {
    request(Method::GET, uri, Body::empty())
}

pub(super) fn request(method: Method, uri: &str, body: Body) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-request-id", REQUEST_ID)
        .body(body)
        .expect("HTTP 请求");
    request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 22], 3123))));
    request
}

pub(super) fn login(request: &mut Request<Body>, user_id: Uuid) {
    request.headers_mut().insert(
        header::COOKIE,
        HeaderValue::from_static(if user_id == SELLER {
            GOOD_COOKIE
        } else {
            "auction_public_session=fixture-other"
        }),
    );
    request.headers_mut().insert(
        PUBLIC_EXPECTED_USER_ID_HEADER,
        HeaderValue::from_str(&user_id.to_string()).expect("预期用户"),
    );
}

pub(super) struct Wire {
    pub(super) status: u16,
    pub(super) headers: HeaderMap,
    pub(super) body: Value,
}

impl Wire {
    pub(super) fn assert(&self, status: u16, code: &str) {
        assert_eq!(self.status, status, "响应分类 {}", self.body["code"]);
        assert_eq!(self.body["code"], code);
        if status >= 400 {
            assert!(self.body["data"].is_null());
            assert!(self.body.get("request_id").is_some());
        }
    }
}

pub(super) async fn send(app: &Router, request: Request<Body>) -> Wire {
    let head = request.method() == Method::HEAD;
    let request_id = request.headers()["x-request-id"].clone();
    let (parts, body) = app.clone().oneshot(request).await.expect("HTTP 响应").into_parts();
    assert_eq!(parts.headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(parts.headers["x-request-id"], request_id);
    assert!(!parts.headers.contains_key(header::ETAG));
    assert!(
        parts.headers.values().all(|value| !value
            .as_bytes()
            .windows(FAKE_KEY.len())
            .any(|part| part == FAKE_KEY.as_bytes())),
        "响应头含读取凭证"
    );
    let bytes = to_bytes(body, 2 << 20).await.expect("HTTP 正文");
    let raw = std::str::from_utf8(&bytes).expect("UTF-8");
    assert!(!raw.contains(FAKE_LINK) && !raw.contains(FAKE_KEY), "响应含读取凭证");
    let body = if head {
        assert!(raw.is_empty());
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("HTTP JSON")
    };
    Wire { status: parts.status.as_u16(), headers: parts.headers, body }
}
