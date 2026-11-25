//! Admin 订单 recheck / resolve 处理器的进程内 HTTP 测试：认证先于业务、动作闭集、回执按处置分形、
//! 确定拒绝的状态码、回执与请求不符按结果未知失败关闭。端口是内存假实现，不连数据库。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, DepositBusinessRejection, ManageListingService,
    OrderAdminService, OrderAdminStore, OrderResolveAction, OrderResolveReceipt,
    RecheckOrderCommand, RecheckOrderReceipt, ResolveOrderCommand,
};
use auction_application::core::{ApplicationFuture, SessionError};
use auction_application::public::{AdminLoginResult, AdminSessionIdentity, AdminSessionPort};
use auction_domain::CommandId;
use auction_time::Timestamp;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use axum::response::Response;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::{AdminOrderWriteCapabilities, AdminOrderWriteState, mount_writes};
use crate::admin::ADMIN_CSRF_HEADER;
use crate::admin::consignment::AdminConsignmentState;
use crate::middleware::security::{AdminHttpSecurity, HttpSecurityOptions};
use crate::state::AdminApiState;

const CONTRACT: Uuid = fixture_uuid(0x10);
const EVIDENCE: Uuid = fixture_uuid(0x13);
const COMMAND: Uuid = fixture_uuid(0x20);
const ACTOR: &str = "fixture-admin";
const ORIGIN: &str = "https://admin.example.invalid";

const fn fixture_uuid(suffix: u128) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 + suffix)
}

/// 假端口的行为：成功、确定拒绝，或回执与请求不符。
#[derive(Clone, Copy)]
enum Mode {
    Succeed,
    Reject(DepositBusinessRejection),
    WrongCommand,
}

struct FakeOrders {
    calls: Mutex<usize>,
    mode: Mutex<Mode>,
}

impl FakeOrders {
    fn new(mode: Mode) -> Self {
        Self { calls: Mutex::new(0), mode: Mutex::new(mode) }
    }

    fn calls(&self) -> usize {
        *self.calls.lock().expect("端口状态")
    }

    fn set(&self, mode: Mode) {
        *self.mode.lock().expect("端口状态") = mode;
    }

    fn receipt_command(&self, command: CommandId) -> Result<CommandId, ConsignmentError> {
        *self.calls.lock().expect("端口状态") += 1;
        match *self.mode.lock().expect("端口状态") {
            Mode::Succeed => Ok(command),
            Mode::Reject(rejection) => Err(ConsignmentError::Deposit(rejection)),
            Mode::WrongCommand => Ok(CommandId::from_uuid(fixture_uuid(0x2f))),
        }
    }
}

impl OrderAdminStore for FakeOrders {
    fn recheck_order(
        &self,
        command: RecheckOrderCommand,
    ) -> ConsignmentFuture<'_, RecheckOrderReceipt> {
        let result =
            self.receipt_command(command.command_id).map(|command_id| RecheckOrderReceipt {
                command_id,
                contract_id: command.contract_id,
                contract_version: command.expected_version,
                work_id: fixture_uuid(0x40),
            });
        Box::pin(async move { result })
    }

    fn resolve_order(
        &self,
        command: ResolveOrderCommand,
    ) -> ConsignmentFuture<'_, OrderResolveReceipt> {
        let result = self.receipt_command(command.command_id).map(|command_id| {
            if command.action == OrderResolveAction::RedispatchOriginal {
                OrderResolveReceipt::RedispatchQueued {
                    command_id,
                    contract_id: command.contract_id,
                    contract_version: command.expected_version,
                    dispatch_id: fixture_uuid(0x41),
                }
            } else {
                OrderResolveReceipt::ResolutionRecorded {
                    command_id,
                    contract_id: command.contract_id,
                    contract_version: command.expected_version + 1,
                    action: command.action,
                    resolution_id: fixture_uuid(0x42),
                }
            }
        });
        Box::pin(async move { result })
    }
}

struct Sessions;

impl Sessions {
    fn authenticate(
        token: &str,
        csrf: &str,
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
    ) -> ApplicationFuture<'_, AdminLoginResult> {
        Box::pin(async { Err(SessionError::Unauthenticated.into()) })
    }

    fn current(
        &self,
        token: String,
        csrf: String,
        now: Timestamp,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        let result = Self::authenticate(&token, &csrf, now).map_err(Into::into);
        Box::pin(async move { result })
    }

    fn touch(
        &self,
        token: String,
        csrf: String,
        now: Timestamp,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        let result = Self::authenticate(&token, &csrf, now).map_err(Into::into);
        Box::pin(async move { result })
    }

    fn logout(&self, _token: String, _now: Timestamp) -> ApplicationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Clone)]
struct TestState {
    admin: AdminApiState,
    writes: AdminOrderWriteCapabilities,
}

impl AdminConsignmentState for TestState {
    fn admin_state(&self) -> &AdminApiState {
        &self.admin
    }

    fn consignment_read(&self) -> Option<&dyn auction_application::consignment::AdminListingRead> {
        None
    }

    fn consignment_manage(&self) -> Option<&ManageListingService> {
        None
    }
}

impl AdminOrderWriteState for TestState {
    fn order_writes(&self) -> &AdminOrderWriteCapabilities {
        &self.writes
    }
}

fn setup(port: &Arc<FakeOrders>) -> Router {
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
    let service = Arc::new(OrderAdminService::new(port.clone()));
    let state = TestState { admin, writes: AdminOrderWriteCapabilities { admin: Some(service) } };
    mount_writes(Router::new()).with_state(state)
}

fn headers(authenticated: bool) -> HeaderMap {
    let mut headers = HeaderMap::from_iter([
        (header::ORIGIN, HeaderValue::from_static(ORIGIN)),
        (header::CONTENT_TYPE, HeaderValue::from_static("application/json")),
    ]);
    // 未认证请求只缺会话 Cookie，CSRF 守卫照常通过，从而验证认证先于业务。
    let cookie = if authenticated {
        "auction_admin_session=session-placeholder; auction_admin_csrf=csrf-placeholder"
    } else {
        "auction_admin_csrf=csrf-placeholder"
    };
    headers.insert(header::COOKIE, HeaderValue::from_static(cookie));
    headers.insert(
        ADMIN_CSRF_HEADER.parse::<axum::http::HeaderName>().expect("CSRF 头"),
        HeaderValue::from_static("csrf-placeholder"),
    );
    headers
}

fn request(method: Method, uri: &str, body: Body, authenticated: bool) -> Request<Body> {
    let mut request = Request::builder().method(method).uri(uri).body(body).expect("HTTP 请求");
    *request.headers_mut() = headers(authenticated);
    request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 23], 3124))));
    request
}

fn path(operation: &str) -> String {
    format!("/api/v2/admin/consignment/orders/{CONTRACT}/{operation}")
}

fn resolve_body(action: &str) -> Value {
    json!({
        "command_id": COMMAND, "expected_version": 3,
        "action": action, "evidence_id": EVIDENCE,
    })
}

async fn send(app: &Router, request: Request<Body>) -> Response {
    app.clone().oneshot(request).await.expect("HTTP 响应")
}

async fn json_body(response: Response, status: StatusCode) -> Value {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let bytes = to_bytes(response.into_body(), 1 << 20).await.expect("HTTP 正文");
    serde_json::from_slice(&bytes).expect("HTTP JSON")
}

fn post(operation: &str, body: &Value, authenticated: bool) -> Request<Body> {
    request(Method::POST, &path(operation), Body::from(body.to_string()), authenticated)
}

fn sorted_keys(reply: &Value) -> Vec<&str> {
    let mut keys: Vec<_> =
        reply["data"].as_object().expect("回执对象").keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys
}

/// 非 Admin 得 401，且从不触达端口（认证先于业务）。
#[tokio::test]
async fn unauthenticated_order_writes_never_reach_the_port() {
    let port = Arc::new(FakeOrders::new(Mode::Succeed));
    let app = setup(&port);
    for (operation, body) in [
        ("recheck", json!({ "command_id": COMMAND, "expected_version": 3 })),
        ("resolve", resolve_body("refund_lost")),
    ] {
        let reply =
            json_body(send(&app, post(operation, &body, false)).await, StatusCode::UNAUTHORIZED)
                .await;
        assert_eq!(reply["code"], "unauthenticated", "{operation}");
    }
    assert_eq!(port.calls(), 0);
}

/// 动作是闭集：未知动作与金额 / 收件人字段整体拒绝且不触达端口；闭集动作按处置分形——
/// 处置记录 200 带动作与 resolution_id，补寄 202 只带 dispatch_id、不带动作。
#[tokio::test]
async fn resolve_accepts_only_the_closed_action_set_and_maps_receipts_by_disposition() {
    let port = Arc::new(FakeOrders::new(Mode::Succeed));
    let app = setup(&port);
    for bad in ["refund", "REFUND_LOST", "refund_all", "payout"] {
        let reply = send(&app, post("resolve", &resolve_body(bad), true)).await;
        assert_eq!(reply.status(), StatusCode::BAD_REQUEST, "{bad}");
    }
    for extra in ["amount", "recipient"] {
        let mut body = resolve_body("refund_lost");
        body[extra] = json!(1);
        let reply = send(&app, post("resolve", &body, true)).await;
        assert_eq!(reply.status(), StatusCode::BAD_REQUEST, "{extra}");
    }
    assert_eq!(port.calls(), 0);

    let recorded = json_body(
        send(&app, post("resolve", &resolve_body("refund_lost"), true)).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        sorted_keys(&recorded),
        [
            "action",
            "command_id",
            "contract_id",
            "contract_version",
            "disposition",
            "operation",
            "resolution_id",
            "schema_version",
        ]
    );
    assert_eq!(recorded["data"]["disposition"], "resolution_recorded");
    assert_eq!(recorded["data"]["contract_version"], 4);

    let queued = json_body(
        send(&app, post("resolve", &resolve_body("redispatch_original"), true)).await,
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(
        sorted_keys(&queued),
        [
            "command_id",
            "contract_id",
            "contract_version",
            "dispatch_id",
            "disposition",
            "operation",
            "schema_version",
        ]
    );
    assert_eq!(queued["data"]["disposition"], "redispatch_queued");
}

/// 确定拒绝的码与状态：相反付款 409、动作不允许 403、版本落后 409；recheck 202 回执只含合同、版本与工作 ID。
#[tokio::test]
async fn rejections_keep_their_codes_and_recheck_returns_the_queued_work() {
    let port =
        Arc::new(FakeOrders::new(Mode::Reject(DepositBusinessRejection::OppositePaymentOpen)));
    let app = setup(&port);
    let error = json_body(
        send(&app, post("resolve", &resolve_body("refund_lost"), true)).await,
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(error["code"], "opposite_payment_open");
    port.set(Mode::Reject(DepositBusinessRejection::ActionNotAllowed));
    let error = json_body(
        send(&app, post("resolve", &resolve_body("cancel_unpaid"), true)).await,
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(error["code"], "action_not_allowed");
    port.set(Mode::Reject(DepositBusinessRejection::StaleVersion));
    let recheck = json!({ "command_id": COMMAND, "expected_version": 3 });
    let error =
        json_body(send(&app, post("recheck", &recheck, true)).await, StatusCode::CONFLICT).await;
    assert_eq!(error["code"], "stale_version");

    port.set(Mode::Succeed);
    let queued =
        json_body(send(&app, post("recheck", &recheck, true)).await, StatusCode::ACCEPTED).await;
    assert_eq!(
        sorted_keys(&queued),
        [
            "command_id",
            "contract_id",
            "contract_version",
            "disposition",
            "operation",
            "schema_version",
            "work_id",
        ]
    );
    assert_eq!(queued["data"]["disposition"], "recheck_queued");
}

/// 回执的命令 ID 与请求不符：写结果无法确认，按结果未知失败关闭，只能原样重试。
#[tokio::test]
async fn a_receipt_for_another_command_is_an_unknown_outcome() {
    let port = Arc::new(FakeOrders::new(Mode::WrongCommand));
    let app = setup(&port);
    let error = json_body(
        send(&app, post("resolve", &resolve_body("refund_lost"), true)).await,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert_eq!(error["code"], "outcome_unknown");
}
