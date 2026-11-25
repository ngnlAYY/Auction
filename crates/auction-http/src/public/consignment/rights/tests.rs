//! 受限权利 HTTP 的核心安全边界：同形响应、Cookie 属性、秘密不外泄、软关闭可用与未认证失败关闭。

use std::fmt;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::consignment::{
    BrowserBinding, ChallengeAad, ChallengeIssued, ChallengeRightCommand, ClaimDisposition,
    ClaimOutcome, ClaimRightCommand, LocateRightCommand, RightChallengeCrypto, RightCryptoError,
    RightReceipt, RightVerified, RightsFuture, RightsService, RightsStore, RightsStoreError,
    SealedChallengeSecret, VerifyRightCommand,
};
use auction_domain::consignment::FeeBps;
use auction_domain::consignment::rights::{
    ChallengeCode, RightKind, RightSessionToken, RightTarget, RightView, SessionDigest,
    format_locator, format_session_token, parse_session_token, session_digest,
};
use auction_domain::{CommandId, SettlementWorld};
use auction_time::Timestamp;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use axum::{Router, middleware};
use serde_json::{Value, json};
use tower::ServiceExt;
use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::{Interest, NoSubscriber};
use tracing::{Dispatch, Event, Metadata, Subscriber};
use uuid::Uuid;

use super::{RightsCapabilities, RightsState, mount};
use crate::middleware::boundary::{HttpBoundary, enforce};
use crate::middleware::security::{HttpSecurityOptions, PublicHttpSecurity};
use crate::public::consignment::{ConsignmentCapabilities, ConsignmentState};
use crate::state::PublicApiState;

const ORIGIN: &str = "https://fixture.invalid";
const VALID_CODE: &str = "0123456789ABCDEFGHJKMNPQRS";
const MALFORMED_CODE: &str = "UUUUU-UUUUU-UUUUU-UUUUU-UUUUU";
const BROWSER_COOKIE: &str = "hv_right_browser=QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI";
const OTHER_BROWSER_COOKIE: &str = "hv_right_browser=Q0NDQ0NDQ0NDQ0NDQ0NDQ0NDQ0NDQ0NDQ0NDQ0NDQ0M";
const RIGHT_ID: Uuid = Uuid::from_u128(0x0000_0000_0000_5000_8000_0000_0000_0521);
const SESSION_ENTROPY: [u8; 32] = [5; 32];

fn locator() -> String {
    format_locator([7; 32]).as_str().to_owned()
}

fn command(suffix: u128) -> String {
    Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 + suffix).to_string()
}

fn challenge_for(target: Target) -> Uuid {
    Uuid::from_u128(0x1111_1111_1111_4111_8111_1111_1111_1111 + target as u128)
}

fn deadline() -> Timestamp {
    Timestamp::parse_from_rfc3339("2026-11-27T18:10:00Z").expect("夹具时刻")
}

fn session_text() -> String {
    format_session_token(&RightSessionToken::from_entropy(SESSION_ENTROPY)).as_str().to_owned()
}

/// 目标种类只影响假存储内部的“真实性”，HTTP 响应不得因它不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Real = 0,
    Virtual = 1,
    Unmatched = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Locate(BrowserBinding),
    Challenge(BrowserBinding),
    Verify(BrowserBinding),
    Claim { right_id: Uuid, expected_version: u64 },
}

/// 内存假存储：只记录调用并按开关返回，不含任何目标真实性之外的业务规则。
struct FakeRights {
    target: Mutex<Target>,
    budget_exhausted: AtomicBool,
    verify_ok: AtomicBool,
    claim_error: Mutex<Option<RightsStoreError>>,
    calls: Mutex<Vec<Call>>,
}

impl FakeRights {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            target: Mutex::new(Target::Real),
            budget_exhausted: AtomicBool::new(false),
            verify_ok: AtomicBool::new(true),
            claim_error: Mutex::new(None),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("调用记录").clone()
    }

    fn record(&self, call: Call) {
        self.calls.lock().expect("调用记录").push(call);
    }

    fn issued(&self) -> Result<ChallengeIssued, RightsStoreError> {
        if self.budget_exhausted.load(Ordering::SeqCst) {
            return Err(RightsStoreError::BudgetExceeded);
        }
        let target = *self.target.lock().expect("目标");
        Ok(ChallengeIssued { challenge_id: challenge_for(target), expires_at: deadline() })
    }

    fn view() -> RightView {
        RightView {
            right_id: RIGHT_ID,
            right_version: 2,
            kind: RightKind::ClaimPayment,
            world: SettlementWorld::Main,
            contract_or_position: RightTarget::Contract(Uuid::from_u128(0x531)),
            allowed_actions: vec![auction_domain::consignment::rights::RightAction::ClaimPayment],
        }
    }
}

fn known_session(session: SessionDigest) -> bool {
    session == session_digest(&RightSessionToken::from_entropy(SESSION_ENTROPY))
}

impl RightsStore for FakeRights {
    fn locate(&self, command: LocateRightCommand) -> RightsFuture<'_, ChallengeIssued> {
        self.record(Call::Locate(command.browser));
        Box::pin(async move { self.issued() })
    }

    fn challenge(&self, command: ChallengeRightCommand) -> RightsFuture<'_, ChallengeIssued> {
        self.record(Call::Challenge(command.browser));
        Box::pin(async move { self.issued() })
    }

    fn verify(&self, command: VerifyRightCommand) -> RightsFuture<'_, RightVerified> {
        self.record(Call::Verify(command.browser));
        Box::pin(async move {
            if self.budget_exhausted.load(Ordering::SeqCst) {
                return Err(RightsStoreError::BudgetExceeded);
            }
            // 真实目标错码、虚拟目标与未匹配目标一律同一个拒绝；只有真实目标且码对才成功。
            let target = *self.target.lock().expect("目标");
            if target != Target::Real || !self.verify_ok.load(Ordering::SeqCst) {
                return Err(RightsStoreError::ChallengeInvalid);
            }
            Ok(RightVerified {
                right_id: RIGHT_ID,
                right_version: 2,
                expires_at: deadline(),
                session_token: RightSessionToken::from_entropy(SESSION_ENTROPY),
            })
        })
    }

    fn current(
        &self,
        session: SessionDigest,
        _browser: BrowserBinding,
    ) -> RightsFuture<'_, Option<RightView>> {
        Box::pin(async move { Ok(known_session(session).then(Self::view)) })
    }

    fn claim(&self, command: ClaimRightCommand) -> RightsFuture<'_, ClaimOutcome> {
        self.record(Call::Claim {
            right_id: command.right_id,
            expected_version: command.expected_version,
        });
        Box::pin(async move {
            if let Some(error) = *self.claim_error.lock().expect("续办错误") {
                return Err(error);
            }
            Ok(ClaimOutcome {
                disposition: ClaimDisposition::ClaimAccepted,
                right_id: command.right_id,
                right_version: 2,
            })
        })
    }

    fn command_receipt(
        &self,
        session: SessionDigest,
        _browser: BrowserBinding,
        command_id: CommandId,
    ) -> RightsFuture<'_, Option<RightReceipt>> {
        Box::pin(async move {
            let known = known_session(session) && command_id.as_uuid().to_string() == command(504);
            Ok(known.then_some(RightReceipt {
                command_id,
                right_id: RIGHT_ID,
                disposition: ClaimDisposition::ClaimAccepted,
            }))
        })
    }
}

struct FakeCrypto;

impl RightChallengeCrypto for FakeCrypto {
    fn seal(
        &self,
        _code: &ChallengeCode,
        _aad: ChallengeAad,
    ) -> Result<SealedChallengeSecret, RightCryptoError> {
        Err(RightCryptoError)
    }

    fn open(
        &self,
        _sealed: &SealedChallengeSecret,
        _aad: ChallengeAad,
    ) -> Result<ChallengeCode, RightCryptoError> {
        Err(RightCryptoError)
    }

    fn mac(&self, _aad: ChallengeAad, _code: &ChallengeCode) -> Result<[u8; 32], RightCryptoError> {
        Ok([9; 32])
    }
}

#[derive(Clone)]
struct TestState {
    base: PublicApiState,
    rights: RightsCapabilities,
    consignment: ConsignmentCapabilities,
}

impl RightsState for TestState {
    fn public_state(&self) -> &PublicApiState {
        &self.base
    }
    fn rights(&self) -> &RightsCapabilities {
        &self.rights
    }
}

impl ConsignmentState for TestState {
    fn public_state(&self) -> &PublicApiState {
        &self.base
    }
    fn consignment(&self) -> &ConsignmentCapabilities {
        &self.consignment
    }
}

fn app(store: &Arc<FakeRights>) -> Router {
    let security = PublicHttpSecurity::new(HttpSecurityOptions {
        cookie_secure: false,
        origins_enabled: true,
        allowed_origins: &[ORIGIN.to_owned()],
        login_limit: 20,
        mutation_limit: 1000,
        anonymous_read_limit: 100,
        anonymous_read_concurrency: 4,
        rate_limit_window: Duration::from_secs(60),
        request_timeout: Duration::from_secs(5),
        trusted_proxy_cidrs: &[],
        trusted_unix_proxy: false,
    })
    .expect("安全策略");
    let base = PublicApiState::new(
        None, None, None, None, None, None, None, None, None, None, None, None, None, None,
        security,
    );
    let service = RightsService::new(store.clone(), Some(Arc::new(FakeCrypto)));
    // 寄售总开关保持关闭：软关闭只截止新承诺，权利路由必须继续可用。
    let state = TestState {
        base,
        rights: RightsCapabilities { service: Some(Arc::new(service)) },
        consignment: ConsignmentCapabilities::without_database(false, FeeBps::DEFAULT_CONSIGNED),
    };
    assert!(!ConsignmentState::consignment(&state).enabled);
    mount(Router::new())
        .with_state(state)
        .layer(middleware::from_fn_with_state(HttpBoundary::default(), enforce))
}

struct Wire {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Wire {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).expect("JSON 正文")
    }

    fn set_cookies(&self) -> Vec<String> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().expect("Cookie 头").to_owned())
            .collect()
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> Wire {
    let mut request = Request::builder().method(method).uri(uri);
    if body.is_some() {
        request =
            request.header(header::ORIGIN, ORIGIN).header(header::CONTENT_TYPE, "application/json");
    }
    let mut request = request
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .expect("HTTP 请求");
    if let Some(cookie) = cookie {
        request
            .headers_mut()
            .insert(header::COOKIE, HeaderValue::from_str(cookie).expect("Cookie"));
    }
    request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 22], 3123))));
    let (parts, body) = app.clone().oneshot(request).await.expect("HTTP 响应").into_parts();
    assert_eq!(parts.headers[header::CACHE_CONTROL], "no-store");
    let bytes = to_bytes(body, 1 << 20).await.expect("HTTP 正文");
    Wire {
        status: parts.status,
        headers: parts.headers,
        body: String::from_utf8(bytes.to_vec()).expect("UTF-8 正文"),
    }
}

async fn post(app: &Router, path: &str, cookie: Option<&str>, body: Value) -> Wire {
    send(app, Method::POST, &format!("/api/v2/consignment/rights/{path}"), cookie, Some(body)).await
}

fn locate_body() -> Value {
    json!({ "command_id": command(501), "game_identity": "FixtureHolder", "world": "main" })
}

fn challenge_body() -> Value {
    json!({ "command_id": command(502), "right_locator": locator() })
}

fn verify_body(code: &str) -> Value {
    json!({ "command_id": command(503), "challenge_id": challenge_for(Target::Real), "code": code })
}

fn claim_body() -> Value {
    json!({ "command_id": command(504), "expected_version": 2, "action": "claim_payment" })
}

/// 去掉按目标不同的挑战 ID 后的整条响应形状：状态、头与正文。
fn shape(wire: &Wire) -> (StatusCode, Vec<(String, String)>, String) {
    let mut body = wire.json();
    if let Some(slot) = body.pointer_mut("/data/challenge_id") {
        let parsed = Uuid::parse_str(slot.as_str().expect("挑战 ID")).expect("挑战 ID");
        assert_eq!(parsed.get_version_num(), 4, "挑战 ID 必须是 v4 形态");
        *slot = json!("<v4>");
    }
    let mut headers: Vec<(String, String)> = wire
        .headers
        .iter()
        .filter(|(name, _)| *name != "content-length" && *name != "x-request-id")
        .map(|(name, value)| (name.to_string(), value.to_str().expect("头").to_owned()))
        .collect();
    headers.sort();
    (wire.status, headers, body.to_string())
}

#[tokio::test]
async fn rights_responses_are_byte_alike_for_real_virtual_and_unmatched_targets() {
    let store = FakeRights::new();
    let app = app(&store);
    for (path, body) in [("locators", locate_body()), ("challenges", challenge_body())] {
        let mut shapes = Vec::new();
        for target in [Target::Real, Target::Virtual, Target::Unmatched] {
            *store.target.lock().expect("目标") = target;
            let wire = post(&app, path, Some(BROWSER_COOKIE), body.clone()).await;
            assert_eq!(wire.status, StatusCode::ACCEPTED, "{path}");
            assert_eq!(wire.json()["data"]["challenge_id"], json!(challenge_for(target)));
            shapes.push(shape(&wire));
        }
        assert!(shapes.windows(2).all(|pair| pair[0] == pair[1]), "{path} 的响应形状随目标变化");
        // 预算用尽的 429 与目标是否真实无关，同样不得出现差异。
        store.budget_exhausted.store(true, Ordering::SeqCst);
        let mut limited = Vec::new();
        for target in [Target::Real, Target::Virtual, Target::Unmatched] {
            *store.target.lock().expect("目标") = target;
            let wire = post(&app, path, Some(BROWSER_COOKIE), body.clone()).await;
            assert_eq!(wire.status, StatusCode::TOO_MANY_REQUESTS, "{path}");
            limited.push(shape(&wire));
        }
        assert!(limited.windows(2).all(|pair| pair[0] == pair[1]), "{path} 的 429 随目标变化");
        store.budget_exhausted.store(false, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn rights_verify_failures_are_indistinguishable_from_malformed_codes() {
    let store = FakeRights::new();
    let app = app(&store);
    store.verify_ok.store(false, Ordering::SeqCst);
    let wrong = post(&app, "verify", Some(BROWSER_COOKIE), verify_body(VALID_CODE)).await;
    let malformed = post(&app, "verify", Some(BROWSER_COOKIE), verify_body(MALFORMED_CODE)).await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    assert_eq!(wrong.json()["code"], "challenge_invalid");
    assert_eq!(shape(&wrong), shape(&malformed));
    // 形状无效的码根本不会到达存储：不存在“错码”与“坏码”的耗时或调用差异之外的信道。
    assert_eq!(store.calls().len(), 1);
    // 真实目标错码、虚拟目标、未匹配目标的核验拒绝字节同形；预算 429 同样与目标无关。
    store.verify_ok.store(true, Ordering::SeqCst);
    for exhausted in [false, true] {
        store.budget_exhausted.store(exhausted, Ordering::SeqCst);
        let mut shapes = Vec::new();
        for (target, ok) in
            [(Target::Real, false), (Target::Virtual, true), (Target::Unmatched, true)]
        {
            *store.target.lock().expect("目标") = target;
            store.verify_ok.store(ok, Ordering::SeqCst);
            let wire = post(&app, "verify", Some(BROWSER_COOKIE), verify_body(VALID_CODE)).await;
            let expected =
                if exhausted { StatusCode::TOO_MANY_REQUESTS } else { StatusCode::FORBIDDEN };
            assert_eq!(wire.status, expected);
            shapes.push(shape(&wire));
        }
        assert!(shapes.windows(2).all(|pair| pair[0] == pair[1]), "verify 的响应形状随目标变化");
    }
}

#[tokio::test]
async fn rights_cookies_carry_required_attributes_and_ten_minute_lifetime() {
    let store = FakeRights::new();
    let app = app(&store);
    let wire = post(&app, "verify", None, verify_body(VALID_CODE)).await;
    assert_eq!(wire.status, StatusCode::OK);
    let cookies = wire.set_cookies();
    assert_eq!(cookies.len(), 2);
    let attributes =
        |cookie: &str| -> Vec<String> { cookie.split("; ").skip(1).map(str::to_owned).collect() };
    let session =
        cookies.iter().find(|cookie| cookie.starts_with("hv_right=")).expect("会话 Cookie");
    assert_eq!(
        attributes(session),
        ["Path=/api/v2/consignment/rights", "Max-Age=600", "HttpOnly", "Secure", "SameSite=Strict"]
    );
    let browser =
        cookies.iter().find(|cookie| cookie.starts_with("hv_right_browser=")).expect("绑定 Cookie");
    let browser_attributes = attributes(browser);
    assert_eq!(browser_attributes[0], "Path=/api/v2/consignment/rights");
    assert_eq!(browser_attributes[2..], ["HttpOnly", "Secure", "SameSite=Strict"]);
    // 令牌只在 Cookie 里，是严格可解析的会话令牌；正文不含它。
    let token = session.split(['=', ';']).nth(1).expect("令牌");
    assert!(parse_session_token(token).is_ok());
    assert!(!wire.body.contains(token));
}

#[tokio::test]
async fn rights_browser_binding_is_stable_per_cookie_and_distinct_across_browsers() {
    let store = FakeRights::new();
    let app = app(&store);
    let first = post(&app, "locators", None, locate_body()).await;
    let minted = first.set_cookies()[0].split(';').next().expect("Cookie 对").to_owned();
    post(&app, "locators", Some(&minted), locate_body()).await;
    post(&app, "locators", Some(OTHER_BROWSER_COOKIE), locate_body()).await;
    post(&app, "locators", None, locate_body()).await;
    let bindings: Vec<_> = store
        .calls()
        .into_iter()
        .map(|call| match call {
            Call::Locate(binding) => binding,
            other => panic!("意外调用 {other:?}"),
        })
        .collect();
    assert_eq!(bindings[0], bindings[1]);
    assert_ne!(bindings[0], bindings[2]);
    assert_ne!(bindings[0], bindings[3], "每个新浏览器必须得到新的随机绑定");
}

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

#[tokio::test]
async fn rights_secrets_are_absent_from_bodies_and_logs() {
    let store = FakeRights::new();
    let app = app(&store);
    let capture = Capture::default();
    // 两个活动分发器让首次在并行测试线程命中的调用点按注册表计算兴趣。
    let _other_dispatch = Dispatch::new(NoSubscriber::default());
    let _subscriber = tracing::subscriber::set_default(capture.clone());
    let token = session_text();
    let both = format!("hv_right={token}; {BROWSER_COOKIE}");
    let mut wires = vec![
        post(&app, "locators", Some(BROWSER_COOKIE), locate_body()).await,
        post(&app, "challenges", Some(BROWSER_COOKIE), challenge_body()).await,
        post(&app, "verify", Some(BROWSER_COOKIE), verify_body(VALID_CODE)).await,
        send(&app, Method::GET, "/api/v2/consignment/rights/current", Some(&both), None).await,
        post(&app, "current/claim", Some(&both), claim_body()).await,
    ];
    wires.push(post(&app, "verify", Some(BROWSER_COOKIE), verify_body(MALFORMED_CODE)).await);
    let browser_value = BROWSER_COOKIE.split('=').nth(1).expect("绑定值");
    let locator = locator();
    let secrets = [locator.as_str(), VALID_CODE, MALFORMED_CODE, token.as_str(), browser_value];
    for wire in &wires {
        for secret in secrets {
            assert!(!wire.body.contains(secret), "正文含秘密");
            for (name, value) in &wire.headers {
                // 核验成功时令牌按设计只出现在 Set-Cookie 里。
                if name != header::SET_COOKIE {
                    assert!(!value.to_str().expect("头").contains(secret), "响应头含秘密");
                }
            }
        }
    }
    let logs = capture.0.lock().expect("日志记录").join("\n");
    for secret in secrets {
        assert!(!logs.contains(secret), "日志含秘密");
    }
}

#[tokio::test]
async fn rights_session_endpoints_fail_closed_as_unauthenticated() {
    let store = FakeRights::new();
    let app = app(&store);
    let current = "/api/v2/consignment/rights/current";
    let receipt = format!("/api/v2/consignment/rights/current/commands/{}", command(504));
    let garbage = format!("hv_right=not-a-token; {BROWSER_COOKIE}");
    let unknown = format!("hv_right=S1-{}; {BROWSER_COOKIE}", "A".repeat(43));
    let only_session = format!("hv_right={}", session_text());
    for cookie in
        [None, Some(garbage.as_str()), Some(unknown.as_str()), Some(only_session.as_str())]
    {
        for uri in [current, receipt.as_str()] {
            let wire = send(&app, Method::GET, uri, cookie, None).await;
            assert_eq!(wire.status, StatusCode::UNAUTHORIZED, "{uri}");
            assert_eq!(wire.json()["code"], "unauthenticated");
        }
        let wire = post(&app, "current/claim", cookie, claim_body()).await;
        assert_eq!(wire.status, StatusCode::UNAUTHORIZED);
        // 缺少或格式错误的会话先于正文解码被拒：连无效正文都得不到 400。
        if cookie != Some(unknown.as_str()) {
            let wire = post(&app, "current/claim", cookie, json!({ "junk": true })).await;
            assert_eq!(wire.status, StatusCode::UNAUTHORIZED);
        }
    }
    assert!(store.calls().is_empty());
}

#[tokio::test]
async fn rights_flow_works_with_consignment_soft_closed_and_claim_maps_outcomes() {
    let store = FakeRights::new();
    let app = app(&store);
    let verified = post(&app, "verify", Some(BROWSER_COOKIE), verify_body(VALID_CODE)).await;
    assert_eq!(verified.status, StatusCode::OK);
    let both = format!("hv_right={}; {BROWSER_COOKIE}", session_text());
    let current =
        send(&app, Method::GET, "/api/v2/consignment/rights/current", Some(&both), None).await;
    assert_eq!(current.status, StatusCode::OK);
    assert_eq!(current.json()["data"]["allowed_actions"], json!(["claim_payment"]));
    let claimed = post(&app, "current/claim", Some(&both), claim_body()).await;
    assert_eq!(claimed.status, StatusCode::ACCEPTED);
    assert_eq!(claimed.json()["data"]["disposition"], "claim_accepted");
    // 续办绑定的是会话对应的权利与读到的版本，而不是请求自报的任何标识。
    assert_eq!(
        store.calls().last(),
        Some(&Call::Claim { right_id: RIGHT_ID, expected_version: 2 })
    );
    let receipt = format!("/api/v2/consignment/rights/current/commands/{}", command(504));
    let replay = send(&app, Method::GET, &receipt, Some(&both), None).await;
    assert_eq!(replay.status, StatusCode::OK);
    let unknown = format!("/api/v2/consignment/rights/current/commands/{}", command(999));
    assert_eq!(
        send(&app, Method::GET, &unknown, Some(&both), None).await.status,
        StatusCode::NOT_FOUND
    );
    *store.claim_error.lock().expect("续办错误") = Some(RightsStoreError::StaleVersion);
    let stale = post(&app, "current/claim", Some(&both), claim_body()).await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(stale.json()["code"], "stale_version");
}
