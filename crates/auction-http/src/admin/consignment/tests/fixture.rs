//! 正式守卫、应用下架用例及可追踪调用的内存端口。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::admin::EquipmentKeySealerPort;
use auction_application::consignment::{
    AdminDelistListingCommand, AdminListingDetail, AdminListingQuery, AdminListingRead,
    AdminListingRecord, ApplyListingLookupCommand, AutoCloseListingCommand, ConsignmentError,
    ConsignmentFuture, ListingAutoCloseEffect, ListingClosed, ListingCommandOutcome,
    ListingCommandStore, ListingLookupEffect, ListingLookupProgress, ListingPriceReceipt,
    ListingReadPage, ListingRegistered, ManageListingService, PrivateListing, PrivateListingView,
    PrivateListingViewKind, RegisterListingCommand, RegistrationReplayCrypto,
    RepriceListingCommand, WithdrawListingCommand,
};
use auction_application::core::{ApplicationError, ApplicationFuture, SessionError};
use auction_application::public::{AdminLoginResult, AdminSessionIdentity, AdminSessionPort};
use auction_domain::consignment::{
    FeeBps, ListingEquipmentIdentity, ListingMode, ListingPrice, ListingReason, ListingState,
    ListingVersion,
};
use auction_time::Timestamp;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use axum::response::Response;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::super::{AdminConsignmentState, mount};
use crate::admin::ADMIN_CSRF_HEADER;
use crate::middleware::security::{AdminHttpSecurity, HttpSecurityOptions};
use crate::server::contract_fixtures::{admin_address, time};
use crate::state::AdminApiState;

pub(super) const LIST_PATH: &str = "/api/v1/admin/consignment/listings";
pub(super) const LISTING: Uuid = Uuid::from_u128(0xcafe);
pub(super) const SELLER: Uuid = Uuid::from_u128(0xabcd);
pub(super) const COMMAND: Uuid = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0cab);
pub(super) const ACTOR: &str = "fixture-admin";
pub(super) const ORIGIN: &str = "https://admin.example.invalid";
pub(super) const REASON: &str = "运营夹具：重复记录";

pub(super) fn detail_path() -> String {
    format!("{LIST_PATH}/{LISTING}")
}

pub(super) fn delist_path() -> String {
    format!("{}/delist", detail_path())
}

pub(super) fn body(reason: &str) -> Value {
    json!({"command_id": COMMAND, "expected_version": 7, "reason": reason})
}

pub(super) fn headers() -> HeaderMap {
    HeaderMap::from_iter([
        (header::ORIGIN, HeaderValue::from_static(ORIGIN)),
        (header::CONTENT_TYPE, HeaderValue::from_static("application/json")),
        (
            header::COOKIE,
            HeaderValue::from_static(
                "auction_admin_session=session-placeholder; auction_admin_csrf=csrf-placeholder",
            ),
        ),
        (ADMIN_CSRF_HEADER.parse().unwrap(), HeaderValue::from_static("csrf-placeholder")),
    ])
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ReadCall {
    List(AdminListingQuery),
    Detail(Uuid),
}

pub(super) struct ReadPort {
    pub(super) calls: Mutex<Vec<ReadCall>>,
    pub(super) record: AdminListingRecord,
    pub(super) detail: Result<Option<AdminListingDetail>, ConsignmentError>,
    pub(super) list_error: Option<ConsignmentError>,
}

impl AdminListingRead for ReadPort {
    fn list_admin(
        &self,
        query: AdminListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<AdminListingRecord>> {
        self.calls.lock().unwrap().push(ReadCall::List(query));
        Box::pin(async move {
            if let Some(error) = self.list_error {
                return Err(error);
            }
            let window = query.listings.window;
            let items = if window.offset() == 0 { vec![self.record.clone()] } else { Vec::new() };
            Ok(ListingReadPage { items, total: 1, window })
        })
    }

    fn admin_detail(&self, listing_id: Uuid) -> ConsignmentFuture<'_, Option<AdminListingDetail>> {
        self.calls.lock().unwrap().push(ReadCall::Detail(listing_id));
        Box::pin(async move { self.detail.clone() })
    }
}

type WriteResult = Result<ListingCommandOutcome<ListingClosed>, ConsignmentError>;

#[derive(Default)]
pub(super) struct CommandPort {
    pub(super) calls: Mutex<Vec<AdminDelistListingCommand>>,
    pub(super) results: Mutex<VecDeque<WriteResult>>,
    pub(super) pending: bool,
}

impl ListingCommandStore for CommandPort {
    fn register<'a>(
        &'a self,
        _command: RegisterListingCommand<'a>,
        _sealer: Option<&'a dyn EquipmentKeySealerPort>,
        _replay_crypto: Option<&'a dyn RegistrationReplayCrypto>,
    ) -> ConsignmentFuture<'a, ListingCommandOutcome<ListingRegistered>> {
        panic!("Admin 不应登记")
    }

    fn reprice(
        &self,
        _command: RepriceListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingPriceReceipt>> {
        panic!("Admin 不应改价")
    }

    fn withdraw(
        &self,
        _command: WithdrawListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
        panic!("Admin 不应以卖家身份取消")
    }

    fn apply_lookup(
        &self,
        _command: ApplyListingLookupCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingLookupEffect>> {
        panic!("Admin 不应提交解析")
    }

    fn auto_close(
        &self,
        _command: AutoCloseListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingAutoCloseEffect>> {
        panic!("HTTP 不应拆分清退和下架")
    }

    fn admin_delist(
        &self,
        command: AdminDelistListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>> {
        self.calls.lock().unwrap().push(command);
        if self.pending {
            return Box::pin(std::future::pending());
        }
        let result =
            self.results.lock().unwrap().pop_front().unwrap_or(Err(ConsignmentError::Unavailable));
        Box::pin(async move { result })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SessionCall {
    Read,
    Touch,
}

#[derive(Default)]
pub(super) struct SessionPort(pub(super) Mutex<Vec<SessionCall>>);

impl SessionPort {
    fn authenticate(
        &self,
        call: SessionCall,
        token: String,
        csrf: String,
        now: Timestamp,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        self.0.lock().unwrap().push(call);
        Box::pin(async move {
            if token != "session-placeholder" || csrf != "csrf-placeholder" {
                return Err(SessionError::Unauthenticated.into());
            }
            Ok(AdminSessionIdentity {
                actor: ACTOR.to_owned(),
                expires_at: now,
                absolute_expires_at: now,
            })
        })
    }
}

impl AdminSessionPort for SessionPort {
    fn login(
        &self,
        _actor: String,
        _secret: Vec<u8>,
        _now: Timestamp,
    ) -> ApplicationFuture<'_, AdminLoginResult> {
        panic!("寄售路由不应登录")
    }

    fn current(
        &self,
        token: String,
        csrf: String,
        now: Timestamp,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        self.authenticate(SessionCall::Read, token, csrf, now)
    }

    fn touch(
        &self,
        token: String,
        csrf: String,
        now: Timestamp,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        self.authenticate(SessionCall::Touch, token, csrf, now)
    }

    fn logout(&self, _token: String, _now: Timestamp) -> ApplicationFuture<'_, ()> {
        Box::pin(async {
            Err(ApplicationError::CapabilityUnavailable("寄售路由不提供退出"))
        })
    }
}

#[derive(Clone)]
pub(super) struct Fixture {
    admin: AdminApiState,
    pub(super) sessions: Arc<SessionPort>,
    pub(super) read: Option<Arc<ReadPort>>,
    pub(super) command: Arc<CommandPort>,
    pub(super) manage: Option<Arc<ManageListingService>>,
}

impl AdminConsignmentState for Fixture {
    fn admin_state(&self) -> &AdminApiState {
        &self.admin
    }

    fn consignment_read(&self) -> Option<&dyn AdminListingRead> {
        self.read.as_deref().map(|port| port as &dyn AdminListingRead)
    }

    fn consignment_manage(&self) -> Option<&ManageListingService> {
        self.manage.as_deref()
    }
}

impl Fixture {
    pub(super) fn new(enabled: bool) -> Self {
        Self::with_limit(enabled, 1000)
    }

    pub(super) fn with_limit(enabled: bool, mutation_limit: u32) -> Self {
        let security = AdminHttpSecurity::new(HttpSecurityOptions {
            cookie_secure: true,
            origins_enabled: true,
            allowed_origins: &[ORIGIN.to_owned()],
            login_limit: 10,
            mutation_limit,
            anonymous_read_limit: 10,
            anonymous_read_concurrency: 2,
            rate_limit_window: Duration::from_secs(60),
            request_timeout: Duration::from_secs(15),
            trusted_proxy_cidrs: &[],
            trusted_unix_proxy: false,
        })
        .unwrap();
        let sessions = Arc::new(SessionPort::default());
        let admin = AdminApiState::new(
            Some(sessions.clone()),
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
        let read = Arc::new(read_port(enabled));
        let command = Arc::new(CommandPort::default());
        let manage = Arc::new(ManageListingService::new(command.clone(), enabled));
        Self { admin, sessions, read: Some(read), command, manage: Some(manage) }
    }

    pub(super) fn router(&self) -> Router {
        mount(Router::new()).with_state(self.clone())
    }

    pub(super) fn read_port(&self) -> &ReadPort {
        self.read.as_deref().unwrap()
    }

    pub(super) fn read_port_mut(&mut self) -> &mut ReadPort {
        Arc::get_mut(self.read.as_mut().unwrap()).unwrap()
    }

    pub(super) fn assert_no_business_calls(&self) {
        assert!(self.read_port().calls.lock().unwrap().is_empty());
        assert!(self.command.calls.lock().unwrap().is_empty());
    }
}

fn read_port(enabled: bool) -> ReadPort {
    let deadline = time() + auction_time::Duration::hours(24);
    let view = if enabled {
        PrivateListingViewKind::Resolving(
            ListingLookupProgress::new(0, Some(time()), None, false, deadline).unwrap(),
        )
    } else {
        PrivateListingViewKind::Closed { reason: ListingReason::ServiceDisabled, equipment: None }
    };
    let private = PrivateListing {
        listing_id: LISTING,
        version: ListingVersion::new(7).unwrap(),
        identity: ListingEquipmentIdentity::new("isekai", "42").unwrap(),
        price: ListingPrice::new(101).unwrap(),
        pricing_version: 1,
        fee_bps: FeeBps::DEFAULT_CONSIGNED,
        created_at: time(),
        updated_at: time(),
        expires_at: deadline,
        view: PrivateListingView::new(ListingMode::Consigned, ListingState::Resolving, view)
            .unwrap(),
    };
    let record =
        AdminListingRecord::new(private, SELLER.into(), "fixture-seller".to_owned(), None, enabled)
            .unwrap();
    let detail = AdminListingDetail {
        record: record.clone(),
        conflicts: Vec::new(),
        conflicts_truncated: false,
    };
    ReadPort { calls: Mutex::default(), record, detail: Ok(Some(detail)), list_error: None }
}

pub(super) async fn send(
    router: Router,
    method: Method,
    path: &str,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let mut request = Request::builder().method(method).uri(path).body(body).unwrap();
    *request.headers_mut() = headers;
    request.extensions_mut().insert(admin_address());
    router.oneshot(request).await.unwrap()
}

pub(super) async fn get(fixture: &Fixture, path: &str) -> Response {
    send(fixture.router(), Method::GET, path, headers(), Body::empty()).await
}

pub(super) async fn delist(fixture: &Fixture, payload: Value) -> Response {
    send(fixture.router(), Method::POST, &delist_path(), headers(), Body::from(payload.to_string()))
        .await
}

pub(super) async fn response(response: Response, status: StatusCode) -> Value {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers().get(header::CACHE_CONTROL).unwrap(), "no-store");
    assert!(response.headers().get(header::ETAG).is_none());
    let bytes = to_bytes(response.into_body(), 128 << 10).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

pub(super) async fn failure(reply: Response, status: StatusCode, code: &str) -> Value {
    let value = response(reply, status).await;
    assert_eq!(value["code"].as_str(), Some(code));
    assert!(value.get("request_id").is_some());
    assert!(value["message"].as_str().is_some_and(|message| !message.is_empty()));
    assert!(value.get("data").is_some_and(Value::is_null));
    value
}
