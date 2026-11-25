//! 订单读取与购买处理器的离线路由测试：真实 Axum 守卫、严格 DTO 与内存端口。
//!
//! 核心规则：本人订单按角色省略金额且 `no-store`；非本人与不存在同形 404；购买重放回原 201、
//! 同 ID 异请求 409；只有确定写前的拒绝才返回可信的 `funding_unavailable`，存储不可用与提交未知保持
//! 各自的 503 码，回执答非所问时按结果未知处理。

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, ListingReadPage, ListingWorld, OrderCore, OrderQuery,
    OrderRole, OwnedOrder, OwnedOrderRead, PurchaseCommand, PurchaseCommandService,
    PurchaseOutcome, PurchaseReceipt, PurchaseStore, RepriceV2Command, RepriceV2Outcome,
    RepriceV2Store,
};
use auction_application::core::{ApplicationFuture, RequestHash, SessionError};
use auction_application::public::{
    PublicLoginResult, PublicSessionIdentity, PublicSessionPort, PublicSessionPreferenceResult,
    SetPublicSessionPreferenceCommand,
};
use auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER;
use auction_domain::consignment::purchase::PurchaseRejection;
use auction_domain::consignment::{
    ListingPrice, ListingVersion, OrderDeliveryStage, OrderKind, OrderLegStage, OrderPaymentStage,
    OrderStages, OrderState,
};
use auction_domain::{ConsignmentContractId, SettlementWorld, UserId};
use auction_time::Timestamp;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, header};
use axum::middleware;
use serde_json::{Value, json};
use tower::ServiceExt;
use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::{Interest, NoSubscriber};
use tracing::{Dispatch, Event, Metadata, Subscriber};
use uuid::Uuid;

use super::orders::{OrderCapabilities, OrderReadState, mount as mount_orders};
use super::purchase::{PurchaseCapabilities, PurchaseState, mount as mount_purchase};
use super::{ConsignmentCapabilities, ConsignmentState};
use crate::middleware::boundary::{HttpBoundary, enforce};
use crate::middleware::security::{HttpSecurityOptions, PublicHttpSecurity};
use crate::state::PublicApiState;

const ORIGIN: &str = "https://fixture.invalid";
const BUYER: Uuid = fixture_uuid(0xb1);
const SELLER: Uuid = fixture_uuid(0xa1);
const OTHER: Uuid = fixture_uuid(0xc1);
const CONTRACT: Uuid = fixture_uuid(0x50);
const LISTING: Uuid = fixture_uuid(0x01);
const EQUIPMENT: Uuid = fixture_uuid(0x60);
const COMMAND: Uuid = fixture_uuid(0x10);
const PRICE: i64 = 101;

const fn fixture_uuid(suffix: u128) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 + suffix)
}

fn core() -> OrderCore {
    OrderCore {
        contract_id: CONTRACT,
        kind: OrderKind::Sale,
        listing_id: LISTING,
        equipment_id: EQUIPMENT,
        world: ListingWorld::Main,
        stages: OrderStages {
            order_state: OrderState::AwaitingPayment,
            delivery: OrderDeliveryStage::Prepared,
            payment: OrderPaymentStage::Unpaid,
            payout: OrderLegStage::Blocked,
            refund: OrderLegStage::NotApplicable,
        },
        version: 1,
        price: Some(ListingPrice::new(PRICE).expect("夹具价格")),
        deadline_at: Some(Timestamp::parse_from_rfc3339("2026-12-01T00:00:00Z").expect("夹具时刻")),
        fee: Some(5),
        seller_amount: Some(96),
        updated_at: Timestamp::parse_from_rfc3339("2026-11-21T00:00:00Z").expect("夹具时刻"),
    }
}

/// 内存订单读端口：买家与卖家各自看到自己角色的订单，其他人为空。
struct Orders;

impl Orders {
    fn view(user: UserId, contract_id: Uuid) -> Option<OwnedOrder> {
        if contract_id != CONTRACT {
            return None;
        }
        match user.as_uuid() {
            id if id == BUYER => Some(OwnedOrder::new(core(), OrderRole::Buyer)),
            id if id == SELLER => Some(OwnedOrder::new(core(), OrderRole::Seller)),
            _ => None,
        }
    }
}

impl OwnedOrderRead for Orders {
    fn list_owned_orders(
        &self,
        user_id: UserId,
        query: OrderQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<OwnedOrder>> {
        Box::pin(async move {
            let items: Vec<OwnedOrder> = Self::view(user_id, CONTRACT).into_iter().collect();
            Ok(ListingReadPage { total: items.len() as u64, items, window: query.window })
        })
    }

    fn owned_order(
        &self,
        user_id: UserId,
        contract_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<OwnedOrder>> {
        Box::pin(async move { Ok(Self::view(user_id, contract_id)) })
    }
}

/// 购买端口的可切换行为。
#[derive(Clone, Copy)]
enum Mode {
    Normal,
    Reject(PurchaseRejection),
    Fail(ConsignmentError),
    /// 回执的命令 ID 与请求不符。
    WrongReceipt,
}

struct Purchases {
    mode: Mutex<Mode>,
    ledger: Mutex<HashMap<Uuid, (RequestHash, PurchaseReceipt)>>,
    calls: Mutex<u32>,
}

impl Purchases {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            mode: Mutex::new(Mode::Normal),
            ledger: Mutex::new(HashMap::new()),
            calls: Mutex::new(0),
        })
    }

    fn set(&self, mode: Mode) {
        *self.mode.lock().expect("模式") = mode;
    }

    fn calls(&self) -> u32 {
        *self.calls.lock().expect("调用数")
    }
}

impl PurchaseStore for Purchases {
    fn replay_purchase(
        &self,
        command: PurchaseCommand,
    ) -> ConsignmentFuture<'_, Option<PurchaseOutcome>> {
        Box::pin(async move { self.replay(&command) })
    }

    fn purchase(&self, command: PurchaseCommand) -> ConsignmentFuture<'_, PurchaseOutcome> {
        Box::pin(async move {
            *self.calls.lock().expect("调用数") += 1;
            match *self.mode.lock().expect("模式") {
                Mode::Reject(rejection) => return Ok(PurchaseOutcome::Rejected(rejection)),
                Mode::Fail(error) => return Err(error),
                Mode::Normal | Mode::WrongReceipt => {}
            }
            let wrong = matches!(*self.mode.lock().expect("模式"), Mode::WrongReceipt);
            let mut ledger = self.ledger.lock().expect("账本");
            if let Some((hash, receipt)) = ledger.get(&command.command_id.as_uuid()) {
                return if *hash == command.request_hash {
                    Ok(PurchaseOutcome::Purchased(PurchaseReceipt {
                        replayed: true,
                        ..receipt.clone()
                    }))
                } else {
                    Err(ConsignmentError::CommandIdConflict)
                };
            }
            let receipt = PurchaseReceipt {
                command_id: if wrong {
                    auction_domain::CommandId::from_uuid(fixture_uuid(0x99))
                } else {
                    command.command_id
                },
                listing_id: command.input.listing_id,
                listing_version: ListingVersion::new(command.input.expected_version.value() + 1)
                    .expect("版本"),
                contract_id: ConsignmentContractId::from_uuid(CONTRACT),
                contract_version: 1,
                equipment_id: EQUIPMENT,
                world: SettlementWorld::Main,
                price: command.input.expected_price,
                order_state: OrderState::AwaitingPayment,
                replayed: false,
            };
            ledger.insert(command.command_id.as_uuid(), (command.request_hash, receipt.clone()));
            Ok(PurchaseOutcome::Purchased(receipt))
        })
    }
}

impl Purchases {
    /// 只重放：账本命中且摘要一致回原回执，摘要不一致冲突，没有记录为空。
    fn replay(
        &self,
        command: &PurchaseCommand,
    ) -> Result<Option<PurchaseOutcome>, ConsignmentError> {
        let ledger = self.ledger.lock().expect("账本");
        match ledger.get(&command.command_id.as_uuid()) {
            Some((hash, receipt)) if *hash == command.request_hash => {
                Ok(Some(PurchaseOutcome::Purchased(PurchaseReceipt {
                    replayed: true,
                    ..receipt.clone()
                })))
            }
            Some(_) => Err(ConsignmentError::CommandIdConflict),
            None => Ok(None),
        }
    }
}

impl RepriceV2Store for Purchases {
    fn reprice(&self, _command: RepriceV2Command) -> ConsignmentFuture<'_, RepriceV2Outcome> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }
}

struct Sessions;

impl PublicSessionPort for Sessions {
    fn current(
        &self,
        token: String,
        _now: Timestamp,
    ) -> ApplicationFuture<'_, PublicSessionIdentity> {
        Box::pin(async move {
            let user_id = match token.as_str() {
                "buyer" => BUYER,
                "seller" => SELLER,
                "other" => OTHER,
                _ => return Err(SessionError::Unauthenticated.into()),
            };
            Ok(PublicSessionIdentity {
                user_id,
                username: "FixtureUser".to_owned(),
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
    v1: ConsignmentCapabilities,
    orders: OrderCapabilities,
    purchase: PurchaseCapabilities,
}

impl ConsignmentState for TestState {
    fn public_state(&self) -> &PublicApiState {
        &self.base
    }

    fn consignment(&self) -> &ConsignmentCapabilities {
        &self.v1
    }
}

impl OrderReadState for TestState {
    fn orders(&self) -> &OrderCapabilities {
        &self.orders
    }
}

impl PurchaseState for TestState {
    fn purchase(&self) -> &PurchaseCapabilities {
        &self.purchase
    }
}

fn router(purchases: &Arc<Purchases>) -> Router {
    router_with(purchases, true)
}

fn router_with(purchases: &Arc<Purchases>, enabled: bool) -> Router {
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
    let state = TestState {
        base,
        v1: ConsignmentCapabilities::default(),
        orders: OrderCapabilities { read: Some(Arc::new(Orders)) },
        purchase: PurchaseCapabilities {
            enabled,
            commands: Some(Arc::new(PurchaseCommandService::new(
                purchases.clone(),
                purchases.clone(),
            ))),
            catalog: None,
        },
    };
    mount_purchase(mount_orders(Router::new()))
        .with_state(state)
        .layer(middleware::from_fn_with_state(HttpBoundary::default(), enforce))
}

fn request(method: Method, uri: &str, body: Body, user: Option<(&str, Uuid)>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-request-id", "fixture-orders")
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some((token, user)) = user {
        builder = builder
            .header(header::COOKIE, format!("auction_public_session={token}"))
            .header(PUBLIC_EXPECTED_USER_ID_HEADER, user.to_string());
    }
    let mut request = builder.body(body).expect("HTTP 请求");
    request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 22], 3123))));
    request
}

fn get(uri: &str, user: (&str, Uuid)) -> Request<Body> {
    request(Method::GET, uri, Body::empty(), Some(user))
}

fn purchase_request(body: &Value) -> Request<Body> {
    request(
        Method::POST,
        &format!("/api/v2/consignment/listings/{LISTING}/purchase"),
        Body::from(body.to_string()),
        Some(("buyer", BUYER)),
    )
}

struct Wire {
    status: u16,
    headers: HeaderMap,
    body: Value,
}

async fn send(app: &Router, request: Request<Body>) -> Wire {
    let (parts, body) = app.clone().oneshot(request).await.expect("HTTP 响应").into_parts();
    assert_eq!(parts.headers[header::CACHE_CONTROL], "no-store");
    let bytes = to_bytes(body, 2 << 20).await.expect("HTTP 正文");
    Wire {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body: serde_json::from_slice(&bytes).expect("HTTP JSON"),
    }
}

fn purchase_body() -> Value {
    json!({"command_id": COMMAND, "expected_version": 5, "expected_price": PRICE})
}

/// 买家与卖家读到各自角色的订单：买家没有手续费与到手金额，卖家守恒；
/// 非本人与不存在的合同得到逐字相同的 404；所有响应 `no-store`。
#[tokio::test]
async fn owned_orders_are_role_scoped_and_non_owner_gets_the_same_404() {
    let app = router(&Purchases::new());
    let uri = format!("/api/v2/me/consignment/orders/{CONTRACT}");

    let buyer = send(&app, get(&uri, ("buyer", BUYER))).await;
    assert_eq!(buyer.status, 200, "{}", buyer.body);
    assert_eq!(buyer.body["data"]["role"], "buyer");
    let data = buyer.body["data"].as_object().expect("订单对象");
    assert!(!data.contains_key("fee") && !data.contains_key("seller_amount"), "买家不见私有金额");

    let seller = send(&app, get(&uri, ("seller", SELLER))).await;
    assert_eq!(seller.status, 200, "{}", seller.body);
    assert_eq!(seller.body["data"]["role"], "seller");
    assert_eq!(seller.body["data"]["fee"], 5);
    assert_eq!(seller.body["data"]["seller_amount"], 96);

    let outsider = send(&app, get(&uri, ("other", OTHER))).await;
    let missing = send(
        &app,
        get(&format!("/api/v2/me/consignment/orders/{}", fixture_uuid(0x77)), ("other", OTHER)),
    )
    .await;
    assert_eq!((outsider.status, missing.status), (404, 404));
    assert_eq!(outsider.body["code"], "listing_not_found");
    assert_eq!(outsider.body["code"], missing.body["code"]);
    assert_eq!(outsider.body["message"], missing.body["message"], "同形：不泄露存在性");
    for header in outsider.headers.values() {
        assert!(!String::from_utf8_lossy(header.as_bytes()).contains(&CONTRACT.to_string()));
    }

    let list = send(&app, get("/api/v2/me/consignment/orders?role=buyer", ("buyer", BUYER))).await;
    assert_eq!(list.status, 200, "{}", list.body);
    assert_eq!(list.body["data"]["total"], 1);
    let none = send(&app, get("/api/v2/me/consignment/orders", ("other", OTHER))).await;
    assert_eq!(none.body["data"]["total"], 0);
}

/// 同命令 ID 同请求重放回原 201，端口只见到同一命令（账本命中）；同 ID 异请求为 409 且不改原结果。
#[tokio::test]
async fn purchase_replay_returns_the_first_201_and_a_different_request_conflicts() {
    let purchases = Purchases::new();
    let app = router(&purchases);
    let first = send(&app, purchase_request(&purchase_body())).await;
    assert_eq!(first.status, 201, "{}", first.body);
    assert_eq!(first.body["data"]["disposition"], "purchased");
    assert_eq!(first.body["data"]["order_state"], "awaiting_payment");
    let again = send(&app, purchase_request(&purchase_body())).await;
    assert_eq!(again.status, 201, "{}", again.body);
    assert_eq!(again.body["data"]["contract_id"], first.body["data"]["contract_id"]);

    let other = json!({"command_id": COMMAND, "expected_version": 5, "expected_price": PRICE + 1});
    let conflict = send(&app, purchase_request(&other)).await;
    assert_eq!(
        (conflict.status, conflict.body["code"].as_str()),
        (409, Some("command_id_conflict"))
    );
    assert_eq!(purchases.ledger.lock().expect("账本").len(), 1, "冲突不产生第二笔成交");
}

/// 只有确定写前的拒绝才返回可信的 `funding_unavailable`；存储不可用与提交未知保持各自的 503 码，
/// 回执答非所问（命令 ID 不符）按结果未知处理，不伪造确定拒绝或成功。
#[tokio::test]
async fn purchase_trusted_503_is_only_the_determined_pre_write_rejection() {
    let purchases = Purchases::new();
    let app = router(&purchases);
    let post = |app: &Router| {
        let app = app.clone();
        async move { send(&app, purchase_request(&purchase_body())).await }
    };

    purchases.set(Mode::Reject(PurchaseRejection::FundingUnavailable));
    let determined = post(&app).await;
    assert_eq!(
        (determined.status, determined.body["code"].as_str()),
        (503, Some("funding_unavailable"))
    );

    purchases.set(Mode::Fail(ConsignmentError::Unavailable));
    let unavailable = post(&app).await;
    assert_eq!(
        (unavailable.status, unavailable.body["code"].as_str()),
        (503, Some("consignment_unavailable"))
    );

    purchases.set(Mode::Fail(ConsignmentError::OutcomeUnknown));
    let unknown = post(&app).await;
    assert_eq!((unknown.status, unknown.body["code"].as_str()), (503, Some("outcome_unknown")));

    purchases.set(Mode::WrongReceipt);
    let wrong = post(&app).await;
    assert_eq!((wrong.status, wrong.body["code"].as_str()), (503, Some("outcome_unknown")));

    purchases.set(Mode::Reject(PurchaseRejection::SelfPurchase));
    let forbidden = post(&app).await;
    assert_eq!((forbidden.status, forbidden.body["code"].as_str()), (403, Some("self_purchase")));
    assert_eq!(purchases.calls(), 5, "每次请求只调用一次存储端口");
}

/// 寄售关闭后：同 ID 同请求回原 201（不变量 1），同 ID 异请求 409，新 ID 才是首次禁用的 404，
/// 且关闭时没有任何新购买被受理。
#[tokio::test]
async fn closed_consignment_replays_the_first_201_and_refuses_only_new_commands() {
    let purchases = Purchases::new();
    let open = router_with(&purchases, true);
    let first = send(&open, purchase_request(&purchase_body())).await;
    assert_eq!(first.status, 201, "{}", first.body);

    let closed = router_with(&purchases, false);
    let again = send(&closed, purchase_request(&purchase_body())).await;
    assert_eq!(again.status, 201, "{}", again.body);
    assert_eq!(again.body["data"]["contract_id"], first.body["data"]["contract_id"]);

    let other = json!({"command_id": COMMAND, "expected_version": 5, "expected_price": PRICE + 1});
    let conflict = send(&closed, purchase_request(&other)).await;
    assert_eq!(
        (conflict.status, conflict.body["code"].as_str()),
        (409, Some("command_id_conflict"))
    );

    let fresh =
        json!({"command_id": fixture_uuid(0x11), "expected_version": 5, "expected_price": PRICE});
    let refused = send(&closed, purchase_request(&fresh)).await;
    assert_eq!(
        (refused.status, refused.body["code"].as_str()),
        (404, Some("consignment_disabled"))
    );
    assert_eq!(purchases.calls(), 1, "关闭后没有新购买进入存储");
    assert_eq!(purchases.ledger.lock().expect("账本").len(), 1);
}

const FAKE_KEY: &str = "fakekey0123456789abcdef";
const FAKE_LINK: &str = "https://fixture.invalid/equip/12345?key=fakekey0123456789abcdef";
const FAKE_CODE: &str = "C1-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);

impl Visit for Capture {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.lock().expect("日志记录").push(format!("{}={value:?}", field.name()));
    }
}

impl Subscriber for Capture {
    fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
        Interest::always()
    }
    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::TRACE)
    }
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, attributes: &Attributes<'_>) -> Id {
        attributes.record(&mut self.clone());
        Id::from_u64(1)
    }
    fn record(&self, _span: &Id, values: &Record<'_>) {
        values.record(&mut self.clone());
    }
    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}
    fn event(&self, event: &Event<'_>) {
        event.record(&mut self.clone());
    }
    fn enter(&self, _span: &Id) {}
    fn exit(&self, _span: &Id) {}
}

/// 请求里带着装备 key / 链接 / 预约码形态的输入时，订单读、购买成功、确定拒绝与未知结果的响应
/// 与 tracing 事件、span 都不含它们（不变量 7）。
#[tokio::test]
async fn secrets_are_absent_from_responses_and_logs_on_every_order_and_purchase_path() {
    let capture = Capture::default();
    let purchases = Purchases::new();
    let app = router(&purchases);
    // 两个活动分发器让并行测试线程里的调用点按注册表计算兴趣，避免被缓存为忽略。
    let _other_dispatch = Dispatch::new(NoSubscriber::default());
    let _subscriber = tracing::subscriber::set_default(capture.clone());

    let mut bodies = Vec::new();
    let mut record = |wire: Wire| {
        let mut text = wire.body.to_string();
        for value in wire.headers.values() {
            text.push_str(&String::from_utf8_lossy(value.as_bytes()));
        }
        bodies.push(text);
        wire.status
    };
    let encoded: String = url::form_urlencoded::byte_serialize(FAKE_LINK.as_bytes()).collect();
    // 订单读：成功、非本人、非法路径与非法查询（值里夹带凭证形态）。
    record(
        send(&app, get(&format!("/api/v2/me/consignment/orders/{CONTRACT}"), ("buyer", BUYER)))
            .await,
    );
    record(
        send(&app, get(&format!("/api/v2/me/consignment/orders/{CONTRACT}"), ("other", OTHER)))
            .await,
    );
    record(
        send(&app, get(&format!("/api/v2/me/consignment/orders/{FAKE_KEY}"), ("buyer", BUYER)))
            .await,
    );
    record(
        send(&app, get(&format!("/api/v2/me/consignment/orders?role={encoded}"), ("buyer", BUYER)))
            .await,
    );
    record(
        send(
            &app,
            get(&format!("/api/v2/me/consignment/orders?state={FAKE_CODE}"), ("buyer", BUYER)),
        )
        .await,
    );
    // 购买：路径夹带凭证、成功、确定拒绝、存储不可用、未知结果。
    let bad_path = request(
        Method::POST,
        &format!("/api/v2/consignment/listings/{FAKE_KEY}/purchase"),
        Body::from(purchase_body().to_string()),
        Some(("buyer", BUYER)),
    );
    record(send(&app, bad_path).await);
    record(send(&app, purchase_request(&purchase_body())).await);
    for mode in [
        Mode::Reject(PurchaseRejection::PriceChanged),
        Mode::Fail(ConsignmentError::Unavailable),
        Mode::Fail(ConsignmentError::OutcomeUnknown),
    ] {
        purchases.set(mode);
        let body = json!({"command_id": fixture_uuid(0x12), "expected_version": 5, "expected_price": PRICE});
        record(send(&app, purchase_request(&body)).await);
    }

    for text in &bodies {
        for secret in [FAKE_KEY, FAKE_LINK, FAKE_CODE] {
            assert!(!text.contains(secret), "响应含秘密：{text}");
        }
    }
    let logs = capture.0.lock().expect("HTTP 日志");
    assert!(logs.iter().any(|field| field.starts_with("route_pattern=")), "没有捕获到日志");
    for secret in [FAKE_KEY, FAKE_LINK, FAKE_CODE] {
        assert!(!logs.iter().any(|field| field.contains(secret)), "日志含秘密 {secret}");
    }
}

/// 受限权利会话（`hv_right`，无 Public 登录会话）不能新购：购买路由经 `mutation_identity`，
/// 权利 Cookie 不是登录凭据，401 且零购买端口调用、零账本行（不变量 2、6）。
#[tokio::test]
async fn restricted_right_session_cannot_purchase() {
    let purchases = Purchases::new();
    let app = router(&purchases);
    let mut req = request(
        Method::POST,
        &format!("/api/v2/consignment/listings/{LISTING}/purchase"),
        Body::from(purchase_body().to_string()),
        None,
    );
    req.headers_mut().insert(
        header::COOKIE,
        axum::http::HeaderValue::from_static(
            "hv_right=fixture-right-token; hv_right_browser=fixture-binding",
        ),
    );
    let reply = send(&app, req).await;
    assert_eq!((reply.status, reply.body["code"].as_str()), (401, Some("unauthenticated")));
    assert_eq!(*purchases.calls.lock().expect("调用数"), 0);
    assert!(purchases.ledger.lock().expect("账本").is_empty());
}
