//! 卖家重列与本人损失回执处理器的离线路由测试：真实 Axum 守卫、严格 DTO 与内存端口。
//!
//! 核心规则：只认 Public 登录会话（受限权利 Cookie 与未登录一律 401 且端口零调用）；重列同 ID 同请求
//! 回原回执、异请求 409；端口只收到会话用户；损失回执非本人与不存在同形 404，响应不含私有字段。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, CustodyRelistStore, DepositBusinessRejection,
    LossCaseState, ManualRelistService, RelistCommand, RelistReceipt, SellerLossCaseRead,
    SellerLossCaseRecord,
};
use auction_application::core::{ApplicationFuture, RequestHash, SessionError};
use auction_application::public::{
    PublicLoginResult, PublicSessionIdentity, PublicSessionPort, PublicSessionPreferenceResult,
    SetPublicSessionPreferenceCommand,
};
use auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER;
use auction_domain::UserId;
use auction_domain::consignment::{ListingPrice, ListingVersion};
use auction_time::Timestamp;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, header};
use axum::middleware;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::{RecoveryCapabilities, RecoveryState, mount};
use crate::middleware::boundary::{HttpBoundary, enforce};
use crate::middleware::security::{HttpSecurityOptions, PublicHttpSecurity};
use crate::public::consignment::{ConsignmentCapabilities, ConsignmentState};
use crate::state::PublicApiState;

const ORIGIN: &str = "https://fixture.invalid";
const SELLER: Uuid = fixture_uuid(0xa1);
const OTHER: Uuid = fixture_uuid(0xc1);
const LISTING: Uuid = fixture_uuid(0x01);
const POSITION: Uuid = fixture_uuid(0x30);
const COMMAND: Uuid = fixture_uuid(0x10);

const fn fixture_uuid(suffix: u128) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 + suffix)
}

/// 内存重列写端：按命令 ID 记账，摘要相同重放原回执、不同冲突；只有挂单卖家能重列。
#[derive(Default)]
struct Relists {
    ledger: Mutex<HashMap<Uuid, (RequestHash, RelistReceipt)>>,
    sellers: Mutex<Vec<Uuid>>,
}

impl Relists {
    fn calls(&self) -> usize {
        self.sellers.lock().expect("调用记录").len()
    }
}

impl CustodyRelistStore for Relists {
    fn replay_relist(
        &self,
        command: RelistCommand,
    ) -> ConsignmentFuture<'_, Option<RelistReceipt>> {
        let ledger = self.ledger.lock().expect("账本");
        let result = match ledger.get(&command.command_id.as_uuid()) {
            Some((hash, receipt)) if *hash == command.request_hash => Ok(Some(receipt.clone())),
            Some(_) => Err(ConsignmentError::CommandIdConflict),
            None => Ok(None),
        };
        Box::pin(async move { result })
    }

    fn relist(&self, command: RelistCommand) -> ConsignmentFuture<'_, RelistReceipt> {
        Box::pin(async move {
            self.sellers.lock().expect("调用记录").push(command.seller_user_id.as_uuid());
            let mut ledger = self.ledger.lock().expect("账本");
            if let Some((hash, receipt)) = ledger.get(&command.command_id.as_uuid()) {
                return if *hash == command.request_hash {
                    Ok(receipt.clone())
                } else {
                    Err(ConsignmentError::CommandIdConflict)
                };
            }
            if command.seller_user_id.as_uuid() != SELLER {
                return Err(ConsignmentError::Deposit(DepositBusinessRejection::ListingNotActive));
            }
            let receipt = RelistReceipt::new(
                command.command_id,
                command.listing_id,
                ListingVersion::new(command.expected_version.value() + 1).expect("版本"),
                ListingPrice::new(101).expect("价格"),
                2,
            );
            ledger.insert(command.command_id.as_uuid(), (command.request_hash, receipt.clone()));
            Ok(receipt)
        })
    }
}

/// 内存损失回执读端：只有卖家本人能读到唯一的案件。
#[derive(Default)]
struct Losses {
    calls: Mutex<u32>,
}

impl SellerLossCaseRead for Losses {
    fn seller_loss_case(
        &self,
        seller_user_id: UserId,
        position_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<SellerLossCaseRecord>> {
        Box::pin(async move {
            *self.calls.lock().expect("调用数") += 1;
            if seller_user_id.as_uuid() != SELLER || position_id != POSITION {
                return Ok(None);
            }
            Ok(Some(SellerLossCaseRecord {
                position_id,
                loss_case_id: fixture_uuid(0x40),
                state: LossCaseState::Reviewing,
                recorded_at: Timestamp::parse_from_rfc3339("2026-11-21T00:00:00Z").expect("时刻"),
                receipt_id: fixture_uuid(0x41),
            }))
        })
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
    recovery: RecoveryCapabilities,
}

impl ConsignmentState for TestState {
    fn public_state(&self) -> &PublicApiState {
        &self.base
    }

    fn consignment(&self) -> &ConsignmentCapabilities {
        &self.v1
    }
}

impl RecoveryState for TestState {
    fn recovery(&self) -> &RecoveryCapabilities {
        &self.recovery
    }
}

fn router(relists: &Arc<Relists>, losses: &Arc<Losses>) -> Router {
    router_with(relists, losses, true)
}

fn router_with(relists: &Arc<Relists>, losses: &Arc<Losses>, enabled: bool) -> Router {
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
        recovery: RecoveryCapabilities {
            enabled,
            relist: Some(Arc::new(ManualRelistService::new(relists.clone()))),
            loss: Some(losses.clone()),
        },
    };
    mount(Router::new())
        .with_state(state)
        .layer(middleware::from_fn_with_state(HttpBoundary::default(), enforce))
}

/// `login` 为 `None` 时不带 Public 会话；`cookie` 另附任意 Cookie（如受限权利会话）。
fn request(method: Method, uri: &str, body: Value, login: Option<(&str, Uuid)>) -> Request<Body> {
    request_with_cookie(method, uri, body, login, None)
}

fn request_with_cookie(
    method: Method,
    uri: &str,
    body: Value,
    login: Option<(&str, Uuid)>,
    cookie: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method.clone())
        .uri(uri)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some((token, user)) = login {
        builder = builder
            .header(header::COOKIE, format!("auction_public_session={token}"))
            .header(PUBLIC_EXPECTED_USER_ID_HEADER, user.to_string());
    }
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    let body = if method == Method::GET { Body::empty() } else { Body::from(body.to_string()) };
    let mut request = builder.body(body).expect("HTTP 请求");
    request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 23], 3123))));
    request
}

async fn send(app: &Router, request: Request<Body>) -> (u16, Value) {
    let (parts, body) = app.clone().oneshot(request).await.expect("HTTP 响应").into_parts();
    assert_eq!(parts.headers[header::CACHE_CONTROL], "no-store");
    let bytes = to_bytes(body, 1 << 20).await.expect("HTTP 正文");
    (parts.status.as_u16(), serde_json::from_slice(&bytes).expect("HTTP JSON"))
}

fn relist_uri() -> String {
    format!("/api/v2/consignment/listings/{LISTING}/relist")
}

fn loss_uri(position: Uuid) -> String {
    format!("/api/v2/me/consignment/positions/{position}/loss-case")
}

fn relist_body(expected_version: i64) -> Value {
    json!({"command_id": COMMAND, "expected_version": expected_version})
}

/// 未登录与只带受限权利 Cookie 的重列、损失回执读取一律 401，两个端口都没有被调用（D9 §5.3：
/// 受限会话不能重列或读其他权利）。
#[tokio::test]
async fn recovery_routes_require_public_login_even_with_a_rights_cookie() {
    let relists = Arc::new(Relists::default());
    let losses = Arc::new(Losses::default());
    let app = router(&relists, &losses);
    let rights = format!("hv_right=fixture-right-session; hv_right_browser={}", "A".repeat(43));
    for cookie in [None, Some(rights.as_str())] {
        let (status, body) = send(
            &app,
            request_with_cookie(Method::POST, &relist_uri(), relist_body(5), None, cookie),
        )
        .await;
        assert_eq!((status, body["code"].as_str()), (401, Some("unauthenticated")), "{cookie:?}");
        let (status, _) = send(
            &app,
            request_with_cookie(Method::GET, &loss_uri(POSITION), Value::Null, None, cookie),
        )
        .await;
        assert_eq!(status, 401, "{cookie:?}");
    }
    assert_eq!(relists.calls(), 0, "未登录不得到达重列端口");
    assert_eq!(*losses.calls.lock().expect("调用数"), 0, "未登录不得到达回执端口");
}

/// 同命令 ID 同请求回原 200 回执、账本只有一笔；同 ID 异请求为 409 且不改原结果。
#[tokio::test]
async fn relist_replays_the_original_receipt_and_a_different_request_conflicts() {
    let relists = Arc::new(Relists::default());
    let app = router(&relists, &Arc::new(Losses::default()));
    let login = Some(("seller", SELLER));
    let (status, first) =
        send(&app, request(Method::POST, &relist_uri(), relist_body(5), login)).await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["data"]["disposition"], "relisted");
    assert_eq!(first["data"]["state"], "available");
    assert_eq!(first["data"]["generation"], 2);
    let (status, again) =
        send(&app, request(Method::POST, &relist_uri(), relist_body(5), login)).await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["data"], first["data"], "重放回原回执");

    let (status, conflict) =
        send(&app, request(Method::POST, &relist_uri(), relist_body(6), login)).await;
    assert_eq!((status, conflict["code"].as_str()), (409, Some("command_id_conflict")));
    assert_eq!(relists.ledger.lock().expect("账本").len(), 1, "冲突不产生第二次重列");
}

/// 非本人挂单的重列被确定拒绝；端口收到的是会话用户而不是任何请求字段。
#[tokio::test]
async fn relist_of_another_sellers_listing_is_rejected() {
    let relists = Arc::new(Relists::default());
    let app = router(&relists, &Arc::new(Losses::default()));
    let (status, body) =
        send(&app, request(Method::POST, &relist_uri(), relist_body(5), Some(("other", OTHER))))
            .await;
    assert_eq!((status, body["code"].as_str()), (409, Some("listing_not_active")));
    assert_eq!(*relists.sellers.lock().expect("调用记录"), [OTHER]);
    assert!(relists.ledger.lock().expect("账本").is_empty());
}

/// 本人读到恰好五个公开字段，没有私有说明、证据或金额；非本人与不存在同形 404。
#[tokio::test]
async fn loss_case_is_owner_only_and_carries_no_private_fields() {
    let app = router(&Arc::new(Relists::default()), &Arc::new(Losses::default()));
    let (status, own) = send(
        &app,
        request(Method::GET, &loss_uri(POSITION), Value::Null, Some(("seller", SELLER))),
    )
    .await;
    assert_eq!(status, 200, "{own}");
    let data = own["data"].as_object().expect("回执对象");
    let mut keys: Vec<&str> = data.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["loss_case_id", "position_id", "receipt_id", "recorded_at", "state"]);

    let (outsider_status, outsider) =
        send(&app, request(Method::GET, &loss_uri(POSITION), Value::Null, Some(("other", OTHER))))
            .await;
    let (missing_status, missing) = send(
        &app,
        request(Method::GET, &loss_uri(fixture_uuid(0x77)), Value::Null, Some(("other", OTHER))),
    )
    .await;
    assert_eq!((outsider_status, missing_status), (404, 404));
    assert_eq!(outsider, missing, "同形：不泄露存在性");
}

/// 寄售关闭后：关闭前已接受的同 ID 同请求重列回原 200 回执（不变量 1），新 ID 才是首次禁用的 404，
/// 且关闭期间没有任何重列进入写端。
#[tokio::test]
async fn closed_consignment_replays_an_accepted_relist_and_refuses_new_commands() {
    let relists = Arc::new(Relists::default());
    let losses = Arc::new(Losses::default());
    let login = Some(("seller", SELLER));
    let open = router(&relists, &losses);
    let (status, first) =
        send(&open, request(Method::POST, &relist_uri(), relist_body(5), login)).await;
    assert_eq!(status, 200, "{first}");

    let closed = router_with(&relists, &losses, false);
    let (status, again) =
        send(&closed, request(Method::POST, &relist_uri(), relist_body(5), login)).await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["data"], first["data"], "关闭后重放回原回执");

    let fresh = json!({"command_id": fixture_uuid(0x11), "expected_version": 5});
    let (status, refused) = send(&closed, request(Method::POST, &relist_uri(), fresh, login)).await;
    assert_eq!((status, refused["code"].as_str()), (404, Some("consignment_disabled")));
    assert_eq!(relists.calls(), 1, "关闭后没有重列进入写端");
    assert_eq!(relists.ledger.lock().expect("账本").len(), 1);
}
