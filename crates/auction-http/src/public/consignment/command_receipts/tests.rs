//! 本人 v2 命令回执查询的离线路由测试：真实 Axum 守卫、严格 DTO 与内存账本。
//!
//! 核心规则：只认 Public 登录会话（未登录与只带受限权利 Cookie 一律 401 且端口零调用）；端口只收到
//! 会话用户，非本人与不存在同形 404；本人回执与原写响应的 `data` 逐字段相同；确定拒绝只给安全码。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentFuture, CustodyRelistStore, ManualRelistService, OwnedCommandReadV2, OwnedCommandV2,
    OwnedDepositDecision, PurchaseOutcome, RelistCommand, RelistReceipt,
};
use auction_application::core::{ApplicationFuture, SessionError};
use auction_application::public::{
    PublicLoginResult, PublicSessionIdentity, PublicSessionPort, PublicSessionPreferenceResult,
    SetPublicSessionPreferenceCommand,
};
use auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER;
use auction_domain::consignment::purchase::PurchaseRejection;
use auction_domain::consignment::{ListingPrice, ListingVersion};
use auction_domain::{CommandId, UserId};
use auction_time::Timestamp;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, header};
use axum::middleware;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::{CommandReceiptCapabilities, CommandReceiptState, mount};
use crate::middleware::boundary::{HttpBoundary, enforce};
use crate::middleware::security::{HttpSecurityOptions, PublicHttpSecurity};
use crate::public::consignment::recovery::{self, RecoveryCapabilities, RecoveryState};
use crate::public::consignment::{ConsignmentCapabilities, ConsignmentState};
use crate::state::PublicApiState;

const ORIGIN: &str = "https://fixture.invalid";
const SELLER: Uuid = fixture_uuid(0xa1);
const OTHER: Uuid = fixture_uuid(0xc1);
const LISTING: Uuid = fixture_uuid(0x01);
const RELIST_COMMAND: Uuid = fixture_uuid(0x10);
const REJECTED_PURCHASE: Uuid = fixture_uuid(0x11);

const fn fixture_uuid(suffix: u128) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 + suffix)
}

/// 内存命令账本：重列写入与回执读取共用，按 actor 隔离；读调用记录会话用户与命令 ID。
#[derive(Default)]
struct Ledger {
    rows: Mutex<HashMap<Uuid, (Uuid, OwnedCommandV2)>>,
    reads: Mutex<Vec<(Uuid, Uuid)>>,
}

impl Ledger {
    fn with_rejected_purchase() -> Arc<Self> {
        let ledger = Self::default();
        ledger.rows.lock().expect("账本").insert(
            REJECTED_PURCHASE,
            (
                SELLER,
                OwnedCommandV2::Purchase(PurchaseOutcome::Rejected(
                    PurchaseRejection::PriceChanged,
                )),
            ),
        );
        Arc::new(ledger)
    }

    fn reads(&self) -> Vec<(Uuid, Uuid)> {
        self.reads.lock().expect("读记录").clone()
    }
}

impl CustodyRelistStore for Ledger {
    fn replay_relist(
        &self,
        _command: RelistCommand,
    ) -> ConsignmentFuture<'_, Option<RelistReceipt>> {
        Box::pin(async { Ok(None) })
    }

    fn relist(&self, command: RelistCommand) -> ConsignmentFuture<'_, RelistReceipt> {
        Box::pin(async move {
            let receipt = RelistReceipt::new(
                command.command_id,
                command.listing_id,
                ListingVersion::new(command.expected_version.value() + 1).expect("版本"),
                ListingPrice::new(101).expect("价格"),
                2,
            );
            self.rows.lock().expect("账本").insert(
                command.command_id.as_uuid(),
                (
                    command.seller_user_id.as_uuid(),
                    OwnedCommandV2::Relist(OwnedDepositDecision::Accepted(receipt.clone())),
                ),
            );
            Ok(receipt)
        })
    }
}

impl OwnedCommandReadV2 for Ledger {
    fn owned_command_v2(
        &self,
        actor_user_id: UserId,
        command_id: CommandId,
    ) -> ConsignmentFuture<'_, Option<OwnedCommandV2>> {
        Box::pin(async move {
            self.reads
                .lock()
                .expect("读记录")
                .push((actor_user_id.as_uuid(), command_id.as_uuid()));
            let rows = self.rows.lock().expect("账本");
            Ok(rows
                .get(&command_id.as_uuid())
                .filter(|(actor, _)| *actor == actor_user_id.as_uuid())
                .map(|(_, command)| command.clone()))
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
    receipts: CommandReceiptCapabilities,
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

impl CommandReceiptState for TestState {
    fn command_receipts(&self) -> &CommandReceiptCapabilities {
        &self.receipts
    }
}

/// 同一账本同时挂重列写路由与回执读路由；寄售开关关闭，回执读取仍须可用。
fn router(ledger: &Arc<Ledger>) -> Router {
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
            enabled: true,
            relist: Some(Arc::new(ManualRelistService::new(ledger.clone()))),
            loss: None,
        },
        receipts: CommandReceiptCapabilities { read: Some(ledger.clone()) },
    };
    mount(recovery::mount(Router::new()))
        .with_state(state)
        .layer(middleware::from_fn_with_state(HttpBoundary::default(), enforce))
}

fn request(
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

fn get(command: Uuid, login: Option<(&str, Uuid)>, cookie: Option<&str>) -> Request<Body> {
    let uri = format!("/api/v2/me/consignment/commands/{command}");
    request(Method::GET, &uri, Value::Null, login, cookie)
}

async fn send(app: &Router, request: Request<Body>) -> (u16, Value) {
    let (parts, body) = app.clone().oneshot(request).await.expect("HTTP 响应").into_parts();
    assert_eq!(parts.headers[header::CACHE_CONTROL], "no-store");
    let bytes = to_bytes(body, 1 << 20).await.expect("HTTP 正文");
    (parts.status.as_u16(), serde_json::from_slice(&bytes).expect("HTTP JSON"))
}

/// 未登录与只带受限权利 Cookie 一律 401，读端口零调用（D9 §5.3：受限会话不能读本人回执）。
#[tokio::test]
async fn command_receipt_requires_a_public_login_even_with_a_rights_cookie() {
    let ledger = Ledger::with_rejected_purchase();
    let app = router(&ledger);
    let rights = format!("hv_right=fixture-right-session; hv_right_browser={}", "A".repeat(43));
    for cookie in [None, Some(rights.as_str())] {
        let (status, body) = send(&app, get(REJECTED_PURCHASE, None, cookie)).await;
        assert_eq!((status, body["code"].as_str()), (401, Some("unauthenticated")), "{cookie:?}");
    }
    assert!(ledger.reads().is_empty(), "未登录不得到达回执端口");
}

/// 非本人与不存在同形 404；端口收到的是会话用户，而不是命令原 actor。
#[tokio::test]
async fn foreign_and_missing_commands_share_one_not_found_shape() {
    let ledger = Ledger::with_rejected_purchase();
    let app = router(&ledger);
    let (foreign_status, foreign) =
        send(&app, get(REJECTED_PURCHASE, Some(("other", OTHER)), None)).await;
    let missing_command = fixture_uuid(0x77);
    let (missing_status, missing) =
        send(&app, get(missing_command, Some(("other", OTHER)), None)).await;
    assert_eq!((foreign_status, missing_status), (404, 404));
    assert_eq!(foreign["code"], "command_not_found");
    assert_eq!(foreign, missing, "同形：不泄露他人命令是否存在");
    assert_eq!(ledger.reads(), [(OTHER, REJECTED_PURCHASE), (OTHER, missing_command)]);
}

/// 本人回执与原写响应的 `data` 逐字段相同；账本没有该命令时为 404，只代表当次未见。
#[tokio::test]
async fn own_relist_receipt_equals_the_original_write_response() {
    let ledger = Arc::new(Ledger::default());
    let app = router(&ledger);
    let login = Some(("seller", SELLER));
    let (status, before) = send(&app, get(RELIST_COMMAND, login, None)).await;
    assert_eq!((status, before["code"].as_str()), (404, Some("command_not_found")));

    let uri = format!("/api/v2/consignment/listings/{LISTING}/relist");
    let body = json!({"command_id": RELIST_COMMAND, "expected_version": 5});
    let (status, written) = send(&app, request(Method::POST, &uri, body, login, None)).await;
    assert_eq!(status, 200, "{written}");
    let (status, read) = send(&app, get(RELIST_COMMAND, login, None)).await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["data"], written["data"], "回执必须与原写响应逐字段相同");
}

/// 已入账的确定拒绝只给安全码、命令 ID 与首次状态，不含目标、版本、价格或任何私有字段。
#[tokio::test]
async fn stored_rejection_carries_only_the_safe_code_and_first_status() {
    let app = router(&Ledger::with_rejected_purchase());
    let (status, body) = send(&app, get(REJECTED_PURCHASE, Some(("seller", SELLER)), None)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["data"],
        json!({
            "schema_version": 2,
            "operation": "purchase",
            "disposition": "rejected",
            "command_id": REJECTED_PURCHASE,
            "code": "price_changed",
            "http_status": 409,
        })
    );
}
