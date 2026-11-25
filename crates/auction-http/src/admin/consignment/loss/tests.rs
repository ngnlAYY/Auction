//! Admin 损失案件处理器的进程内 HTTP 测试：认证先于业务、版本不符、同命令重放与冲突、
//! 回执与读模型不带金额字段。端口是带版本与命令账本语义的内存假实现，不连数据库。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::consignment::{
    AdminLossCaseRecord, AdminLossOutcomeRecord, ConfirmPositionLossCommand, ConsignmentError,
    ConsignmentFuture, DepositBusinessRejection, LossCaseAdmin, LossCaseAdminService,
    LossCaseOutcomeReceipt, LossCaseState, LossProofReceipt, LossReconcileRecord,
    ManageListingService, PositionLossConfirmService, PositionLossReceipt, PositionLossStore,
    RecordLossCaseOutcomeCommand, RecordLossProofCommand,
};
use auction_application::core::RequestHash;
use auction_application::core::{ApplicationFuture, SessionError};
use auction_application::public::{AdminLoginResult, AdminSessionIdentity, AdminSessionPort};
use auction_domain::CommandId;
use auction_domain::consignment::ListingVersion;
use auction_time::Timestamp;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use axum::response::Response;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::{AdminLossCapabilities, AdminLossState, mount};
use crate::admin::ADMIN_CSRF_HEADER;
use crate::admin::consignment::AdminConsignmentState;
use crate::middleware::security::{AdminHttpSecurity, HttpSecurityOptions};
use crate::state::AdminApiState;

const POSITION: Uuid = fixture_uuid(0x10);
const MISSING: Uuid = fixture_uuid(0x11);
const CASE: Uuid = fixture_uuid(0x12);
const BASIS: Uuid = fixture_uuid(0x13);
const COMMAND: Uuid = fixture_uuid(0x20);
const OTHER_COMMAND: Uuid = fixture_uuid(0x21);
const ACTOR: &str = "fixture-admin";
const ORIGIN: &str = "https://admin.example.invalid";
const NOTE: &str = "运营夹具：复核中";

const fn fixture_uuid(suffix: u128) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 + suffix)
}

fn time() -> Timestamp {
    Timestamp::parse_from_rfc3339("2026-11-21T00:00:00Z").expect("夹具时刻")
}

/// 带案件版本与命令账本语义的内存端口：同命令同请求返回原回执，异请求冲突，版本落后为 stale_version。
struct FakeCases {
    state: Mutex<FakeState>,
}

struct FakeState {
    version: u64,
    seen: HashMap<Uuid, (RecordLossCaseOutcomeCommandKey, LossCaseOutcomeReceipt)>,
    calls: usize,
    corrupt_receipt: bool,
    wrong_command: bool,
}

type RecordLossCaseOutcomeCommandKey = (Uuid, u64, &'static str, Uuid, String);

impl FakeCases {
    fn new() -> Self {
        Self {
            state: Mutex::new(FakeState {
                version: 1,
                seen: HashMap::new(),
                calls: 0,
                corrupt_receipt: false,
                wrong_command: false,
            }),
        }
    }

    fn calls(&self) -> usize {
        self.state.lock().expect("端口状态").calls
    }

    fn version(&self) -> u64 {
        self.state.lock().expect("端口状态").version
    }
}

impl LossCaseAdmin for FakeCases {
    fn open_loss_reconciles(
        &self,
        _position_id: Uuid,
    ) -> ConsignmentFuture<'_, Vec<LossReconcileRecord>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn admin_loss_case(
        &self,
        position_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<AdminLossCaseRecord>> {
        let mut state = self.state.lock().expect("端口状态");
        state.calls += 1;
        let version = state.version;
        drop(state);
        Box::pin(async move {
            if position_id != POSITION {
                return Ok(None);
            }
            let opened = AdminLossOutcomeRecord {
                outcome_id: fixture_uuid(0x30),
                case_version: 1,
                state: LossCaseState::Opened,
                basis_id: None,
                private_note: None,
                occurred_at: time(),
                seller_receipt_id: fixture_uuid(0x31),
            };
            let mut outcomes = vec![opened];
            if version > 1 {
                outcomes.push(AdminLossOutcomeRecord {
                    outcome_id: fixture_uuid(0x32),
                    case_version: 2,
                    state: LossCaseState::Reviewing,
                    basis_id: Some(BASIS),
                    private_note: Some(NOTE.to_owned()),
                    occurred_at: time(),
                    seller_receipt_id: fixture_uuid(0x33),
                });
            }
            Ok(Some(AdminLossCaseRecord {
                position_id,
                case_id: CASE,
                loss_fact_id: fixture_uuid(0x34),
                proof_id: fixture_uuid(0x35),
                proof_source_kind: "fake-source".to_owned(),
                proof_digest: [0xab; 32],
                listing_id: fixture_uuid(0x36),
                seller_user_id: fixture_uuid(0x37),
                generation: 1,
                case_version: version,
                state: if version > 1 { LossCaseState::Reviewing } else { LossCaseState::Opened },
                outcomes,
            }))
        })
    }

    fn record_loss_case_outcome(
        &self,
        command: RecordLossCaseOutcomeCommand,
    ) -> ConsignmentFuture<'_, LossCaseOutcomeReceipt> {
        let mut state = self.state.lock().expect("端口状态");
        state.calls += 1;
        let key = (
            command.case_id,
            command.expected_case_version,
            command.outcome.as_str(),
            command.basis_id,
            command.private_note.as_str().to_owned(),
        );
        let result = if let Some((first, receipt)) = state.seen.get(&command.command_id.as_uuid()) {
            if *first == key {
                Ok(receipt.clone())
            } else {
                Err(ConsignmentError::CommandIdConflict)
            }
        } else if command.expected_case_version != state.version {
            Err(ConsignmentError::Deposit(DepositBusinessRejection::StaleVersion))
        } else {
            state.version += 1;
            let mut receipt = LossCaseOutcomeReceipt::new(
                command.command_id,
                command.case_id,
                state.version,
                fixture_uuid(0x32),
                fixture_uuid(0x33),
            );
            state.seen.insert(command.command_id.as_uuid(), (key, receipt.clone()));
            if state.corrupt_receipt {
                receipt.case_version += 5;
            }
            if state.wrong_command {
                receipt.command_id = CommandId::from_uuid(fixture_uuid(0x2f));
            }
            Ok(receipt)
        };
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
    loss: AdminLossCapabilities,
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

impl AdminLossState for TestState {
    fn loss(&self) -> &AdminLossCapabilities {
        &self.loss
    }
}

fn setup(port: &Arc<FakeCases>) -> Router {
    setup_with(port, &Arc::new(FakeConfirms::default()))
}

fn setup_with(port: &Arc<FakeCases>, confirms: &Arc<FakeConfirms>) -> Router {
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
    let service = Arc::new(LossCaseAdminService::new(port.clone()));
    let confirm = Arc::new(PositionLossConfirmService::new(confirms.clone()));
    let state = TestState {
        admin,
        loss: AdminLossCapabilities { cases: Some(service), confirm: Some(confirm) },
    };
    mount(Router::new()).with_state(state)
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

fn case_path(position: Uuid) -> String {
    format!("/api/v2/admin/consignment/positions/{position}/loss-case")
}

fn outcome_path(position: Uuid) -> String {
    format!("{}/outcomes", case_path(position))
}

fn outcome_body(command: Uuid, expected_version: u64) -> Value {
    json!({
        "command_id": command,
        "case_id": CASE,
        "expected_case_version": expected_version,
        "outcome": "reviewing",
        "basis_id": BASIS,
        "private_note": NOTE,
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

#[tokio::test]
async fn unauthenticated_requests_never_reach_the_case_port() {
    let port = Arc::new(FakeCases::new());
    let app = setup(&port);
    let read = request(Method::GET, &case_path(POSITION), Body::empty(), false);
    let reply = json_body(send(&app, read).await, StatusCode::UNAUTHORIZED).await;
    assert_eq!(reply["code"], "unauthenticated");
    let body = Body::from(outcome_body(COMMAND, 1).to_string());
    let write = request(Method::POST, &outcome_path(POSITION), body, false);
    json_body(send(&app, write).await, StatusCode::UNAUTHORIZED).await;
    assert_eq!(port.calls(), 0);
}

#[tokio::test]
async fn unknown_position_is_not_found_and_case_view_has_no_money_fields() {
    let port = Arc::new(FakeCases::new());
    let app = setup(&port);
    let missing = request(Method::GET, &case_path(MISSING), Body::empty(), true);
    json_body(send(&app, missing).await, StatusCode::NOT_FOUND).await;

    let found = request(Method::GET, &case_path(POSITION), Body::empty(), true);
    let reply = json_body(send(&app, found).await, StatusCode::OK).await;
    let data = reply["data"].as_object().expect("案件对象");
    let mut keys: Vec<_> = data.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "case_id",
            "case_version",
            "generation",
            "listing_id",
            "loss_fact_id",
            "outcomes",
            "position_id",
            "proof_digest",
            "proof_id",
            "proof_source_kind",
            "seller_user_id",
            "state",
        ]
    );
    assert_eq!(data["state"], "opened");
}

#[tokio::test]
async fn outcome_receipt_has_no_money_fields_and_replays_the_original() {
    let port = Arc::new(FakeCases::new());
    let app = setup(&port);
    let first = request(
        Method::POST,
        &outcome_path(POSITION),
        Body::from(outcome_body(COMMAND, 1).to_string()),
        true,
    );
    let reply = json_body(send(&app, first).await, StatusCode::OK).await;
    let data = reply["data"].as_object().expect("回执对象");
    let mut keys: Vec<_> = data.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "case_id",
            "case_version",
            "command_id",
            "disposition",
            "operation",
            "outcome_id",
            "schema_version",
            "seller_receipt_id",
        ]
    );
    assert_eq!(data["case_version"], 2);
    assert_eq!(data["disposition"], "case_outcome_recorded");

    // 同 ID 同请求重放：返回原回执，案件版本不再增长。
    let replay = request(
        Method::POST,
        &outcome_path(POSITION),
        Body::from(outcome_body(COMMAND, 1).to_string()),
        true,
    );
    let again = json_body(send(&app, replay).await, StatusCode::OK).await;
    assert_eq!(again["data"], reply["data"]);
    assert_eq!(port.version(), 2);

    // 同 ID 异请求：冲突，不写。
    let mut changed = outcome_body(COMMAND, 1);
    changed["outcome"] = json!("closed");
    let conflict =
        request(Method::POST, &outcome_path(POSITION), Body::from(changed.to_string()), true);
    let error = json_body(send(&app, conflict).await, StatusCode::CONFLICT).await;
    assert_eq!(error["code"], "command_id_conflict");
    assert_eq!(port.version(), 2);
}

#[tokio::test]
async fn stale_case_version_is_rejected_without_appending() {
    let port = Arc::new(FakeCases::new());
    let app = setup(&port);
    let first = request(
        Method::POST,
        &outcome_path(POSITION),
        Body::from(outcome_body(COMMAND, 1).to_string()),
        true,
    );
    json_body(send(&app, first).await, StatusCode::OK).await;
    // 另一条新命令仍按版本 1 提交：案件已是版本 2。
    let stale = request(
        Method::POST,
        &outcome_path(POSITION),
        Body::from(outcome_body(OTHER_COMMAND, 1).to_string()),
        true,
    );
    let error = json_body(send(&app, stale).await, StatusCode::CONFLICT).await;
    assert_eq!(error["code"], "stale_version");
    assert_eq!(port.version(), 2);
}

#[tokio::test]
async fn money_fields_and_mismatched_receipts_never_succeed() {
    let port = Arc::new(FakeCases::new());
    let app = setup(&port);
    let mut body = outcome_body(COMMAND, 1);
    body["amount"] = json!(1);
    let rejected =
        request(Method::POST, &outcome_path(POSITION), Body::from(body.to_string()), true);
    let reply = send(&app, rejected).await;
    assert_eq!(reply.status(), StatusCode::BAD_REQUEST);
    assert_eq!(port.calls(), 0);

    // 回执版本与请求加一不符：按结果未知失败关闭，只能原样重试。
    port.state.lock().expect("端口状态").corrupt_receipt = true;
    let corrupt = request(
        Method::POST,
        &outcome_path(POSITION),
        Body::from(outcome_body(OTHER_COMMAND, 1).to_string()),
        true,
    );
    let error = json_body(send(&app, corrupt).await, StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(error["code"], "outcome_unknown");

    // 回执 command_id 与请求不符：同样按结果未知失败关闭。
    {
        let mut state = port.state.lock().expect("端口状态");
        state.corrupt_receipt = false;
        state.wrong_command = true;
    }
    let expected = port.version();
    let wrong = request(
        Method::POST,
        &outcome_path(POSITION),
        Body::from(outcome_body(fixture_uuid(0x22), expected).to_string()),
        true,
    );
    let error = json_body(send(&app, wrong).await, StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(error["code"], "outcome_unknown");
}

/// 带命令账本语义的灭失写端：同 ID 同摘要回原回执，异摘要冲突；实现证据录入与确认命令。
#[derive(Default)]
struct FakeConfirms {
    seen: Mutex<HashMap<Uuid, (RequestHash, PositionLossReceipt)>>,
    calls: Mutex<usize>,
    proofs: Mutex<HashMap<Uuid, (RequestHash, LossProofReceipt)>>,
    proof_calls: Mutex<usize>,
}

impl PositionLossStore for FakeConfirms {
    fn record_loss_proof(
        &self,
        command: RecordLossProofCommand,
    ) -> ConsignmentFuture<'_, LossProofReceipt> {
        *self.proof_calls.lock().expect("调用数") += 1;
        let mut proofs = self.proofs.lock().expect("账本");
        let result = match proofs.get(&command.command_id.as_uuid()) {
            Some((hash, receipt)) if *hash == command.request_hash => Ok(receipt.clone()),
            Some(_) => Err(ConsignmentError::CommandIdConflict),
            None => {
                let receipt = LossProofReceipt::new(
                    command.command_id,
                    command.position_id,
                    fixture_uuid(0x35),
                    [0xcd; 32],
                );
                proofs
                    .insert(command.command_id.as_uuid(), (command.request_hash, receipt.clone()));
                Ok(receipt)
            }
        };
        Box::pin(async move { result })
    }

    fn confirm_position_loss(
        &self,
        command: ConfirmPositionLossCommand,
    ) -> ConsignmentFuture<'_, PositionLossReceipt> {
        *self.calls.lock().expect("调用数") += 1;
        let mut seen = self.seen.lock().expect("账本");
        let result = match seen.get(&command.command_id.as_uuid()) {
            Some((hash, receipt)) if *hash == command.request_hash => Ok(receipt.clone()),
            Some(_) => Err(ConsignmentError::CommandIdConflict),
            None => {
                let receipt = PositionLossReceipt::new(
                    command.command_id,
                    fixture_uuid(0x36),
                    ListingVersion::new(8).expect("版本"),
                    command.position_id,
                    command.expected_version + 1,
                    fixture_uuid(0x34),
                    CASE,
                    fixture_uuid(0x31),
                );
                seen.insert(command.command_id.as_uuid(), (command.request_hash, receipt.clone()));
                Ok(receipt)
            }
        };
        Box::pin(async move { result })
    }

    fn record_loss_case_outcome(
        &self,
        _command: RecordLossCaseOutcomeCommand,
    ) -> ConsignmentFuture<'_, LossCaseOutcomeReceipt> {
        Box::pin(async { Err(ConsignmentError::Unavailable) })
    }
}

fn confirm_path(position: Uuid) -> String {
    format!("/api/v2/admin/consignment/positions/{position}/confirm-loss")
}

fn confirm_body(command: Uuid, expected_version: u64) -> Value {
    json!({
        "command_id": command,
        "expected_version": expected_version,
        "proof_id": fixture_uuid(0x35),
        "reason": NOTE,
    })
}

/// 无会话的灭失确认 401 且不到达写端；有会话的同 ID 同请求回原回执、只记一笔，异请求 409；
/// 回执不回显私有说明，也没有合同、退款或金额字段。
#[tokio::test]
async fn confirm_loss_requires_a_session_and_replays_the_original_receipt() {
    let confirms = Arc::new(FakeConfirms::default());
    let app = setup_with(&Arc::new(FakeCases::new()), &confirms);
    let body = || Body::from(confirm_body(COMMAND, 3).to_string());
    let anonymous = request(Method::POST, &confirm_path(POSITION), body(), false);
    json_body(send(&app, anonymous).await, StatusCode::UNAUTHORIZED).await;
    assert_eq!(*confirms.calls.lock().expect("调用数"), 0);

    let first = request(Method::POST, &confirm_path(POSITION), body(), true);
    let reply = json_body(send(&app, first).await, StatusCode::OK).await;
    let data = reply["data"].as_object().expect("回执对象");
    let mut keys: Vec<_> = data.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "command_id",
            "disposition",
            "listing_id",
            "listing_version",
            "loss_case_id",
            "loss_fact_id",
            "operation",
            "position_id",
            "position_version",
            "schema_version",
            "seller_receipt_id",
            "state",
        ]
    );
    assert_eq!(data["disposition"], "position_loss_recorded");
    assert!(!reply.to_string().contains(NOTE), "私有说明不得回显");

    let replay = request(Method::POST, &confirm_path(POSITION), body(), true);
    let again = json_body(send(&app, replay).await, StatusCode::OK).await;
    assert_eq!(again["data"], reply["data"]);
    assert_eq!(confirms.seen.lock().expect("账本").len(), 1);

    let changed = Body::from(confirm_body(COMMAND, 4).to_string());
    let conflict = request(Method::POST, &confirm_path(POSITION), changed, true);
    let error = json_body(send(&app, conflict).await, StatusCode::CONFLICT).await;
    assert_eq!(error["code"], "command_id_conflict");
    assert_eq!(confirms.seen.lock().expect("账本").len(), 1);
}

const OBSERVATION: &str = "fake-observation-1";

fn proof_path(position: Uuid) -> String {
    format!("/api/v2/admin/consignment/positions/{position}/loss-proofs")
}

fn proof_body(command: Uuid, source_kind: &str) -> Value {
    json!({
        "command_id": command,
        "source_kind": source_kind,
        "observation_ref": OBSERVATION,
        "last_known_ref": "夹具：Bot 收件箱",
        "private_note": NOTE,
    })
}

/// 证据录入的写守卫先于业务：无会话 401、Origin 不符 403、缺 CSRF 403，观察身份含 key 形状 400；
/// 四者都不触达灭失写端（不变量 6、7）。
#[tokio::test]
async fn loss_proof_guards_reject_before_the_write_port() {
    let confirms = Arc::new(FakeConfirms::default());
    let app = setup_with(&Arc::new(FakeCases::new()), &confirms);
    let body = || Body::from(proof_body(COMMAND, "fake-source").to_string());

    let anonymous = request(Method::POST, &proof_path(POSITION), body(), false);
    json_body(send(&app, anonymous).await, StatusCode::UNAUTHORIZED).await;

    let mut wrong_origin = request(Method::POST, &proof_path(POSITION), body(), true);
    wrong_origin
        .headers_mut()
        .insert(header::ORIGIN, HeaderValue::from_static("https://other.example.invalid"));
    let reply = send(&app, wrong_origin).await;
    assert_eq!(reply.status(), StatusCode::FORBIDDEN);
    let bytes = to_bytes(reply.into_body(), 1 << 20).await.expect("HTTP 正文");
    let error: Value = serde_json::from_slice(&bytes).expect("HTTP JSON");
    assert_eq!(error["code"], "origin_forbidden");

    let mut no_csrf = request(Method::POST, &proof_path(POSITION), body(), true);
    no_csrf.headers_mut().remove(ADMIN_CSRF_HEADER);
    let reply = send(&app, no_csrf).await;
    assert_eq!(reply.status(), StatusCode::FORBIDDEN);
    let bytes = to_bytes(reply.into_body(), 1 << 20).await.expect("HTTP 正文");
    let error: Value = serde_json::from_slice(&bytes).expect("HTTP JSON");
    assert_eq!(error["code"], "csrf_invalid");

    let mut keyed = proof_body(COMMAND, "fake-source");
    keyed["observation_ref"] = json!("seen 0123456789");
    let keyed = request(Method::POST, &proof_path(POSITION), Body::from(keyed.to_string()), true);
    json_body(send(&app, keyed).await, StatusCode::BAD_REQUEST).await;

    assert_eq!(*confirms.proof_calls.lock().expect("调用数"), 0);
    assert!(confirms.proofs.lock().expect("账本").is_empty());
}

/// 同 ID 同请求回原回执、只记一笔；同 ID 异请求 409 且不新增；回执只有 position、证据 ID 与摘要，
/// 不回显私有说明与证据原文（不变量 1、7）。
#[tokio::test]
async fn loss_proof_replays_the_original_receipt_without_echoing_the_note() {
    let confirms = Arc::new(FakeConfirms::default());
    let app = setup_with(&Arc::new(FakeCases::new()), &confirms);
    let send_proof = |source_kind: &'static str| {
        let body = Body::from(proof_body(COMMAND, source_kind).to_string());
        request(Method::POST, &proof_path(POSITION), body, true)
    };

    let reply = json_body(send(&app, send_proof("fake-source")).await, StatusCode::OK).await;
    let data = reply["data"].as_object().expect("回执对象");
    let mut keys: Vec<_> = data.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "command_id",
            "disposition",
            "operation",
            "position_id",
            "proof_digest",
            "proof_id",
            "schema_version",
        ]
    );
    assert_eq!(data["disposition"], "loss_proof_recorded");
    assert_eq!(data["position_id"], json!(POSITION));
    let text = reply.to_string();
    assert!(!text.contains(NOTE) && !text.contains(OBSERVATION), "私有说明与证据原文不得回显");

    let again = json_body(send(&app, send_proof("fake-source")).await, StatusCode::OK).await;
    assert_eq!(again["data"], reply["data"]);
    assert_eq!(confirms.proofs.lock().expect("账本").len(), 1);

    let error = json_body(send(&app, send_proof("other-source")).await, StatusCode::CONFLICT).await;
    assert_eq!(error["code"], "command_id_conflict");
    assert_eq!(confirms.proofs.lock().expect("账本").len(), 1);
    assert_eq!(*confirms.proof_calls.lock().expect("调用数"), 3);
}
