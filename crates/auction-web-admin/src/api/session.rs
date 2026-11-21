//! Admin 会话 HTTP 适配。
//! 浏览器只持有服务端返回的 actor、CSRF token 和过期时间；Admin secret 只用于一次 Basic
//! Auth 请求，不进入组件之外的状态，也不写入日志或 DTO。

use auction_contracts::{AdminSessionResponse, ApiResponse};
use auction_time::Timestamp;
use auction_web_shared::clock::monotonic_millis;
use auction_web_shared::storage::read_cookie;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use dioxus::{
    core::Runtime,
    prelude::{EventHandler, Memo, ReadableExt, try_consume_context},
};
use gloo_net::http::Request;
use serde_json::Value;

use super::failure::read_response_failure;
use super::transport::AdminResponse;
use super::{
    AdminApiFailure, invalid_response_error, response_error, response_failure, send_get,
    send_request,
};

const SESSION_WITH_SERVER_TIME: &str = "/api/v1/admin/session?include=server_time";

/// 当前认证外壳拥有的会话检查触发器；API 只发出重读请求，不据网络错误推断已登出。
#[derive(Clone, Copy)]
pub(crate) struct AdminSessionCheckTrigger(pub(crate) EventHandler<()>);

/// 认证 owner 提供的实时写门禁；临时会话检查失败不会销毁旧草稿，但必须停止新请求。
#[derive(Clone, Copy)]
pub(crate) struct AdminSessionWritePermission(pub(crate) Memo<bool>);

/// 请求发出时记录代际，异代正文与可信 401 都交回原宿主。
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct AdminSessionWriteReview {
    pub(crate) epoch: u64,
    pub(crate) current_epoch: Memo<u64>,
    pub(crate) observe: EventHandler<(u64, AdminSession)>,
    pub(crate) invalidate: EventHandler<(u64, AdminSession)>,
}

/// 读取管理端的可读 CSRF Cookie，保留无 Cookie 与不可读的区别。
pub(crate) fn admin_session_cookie() -> Option<Option<String>> {
    read_cookie("auction_admin_csrf")
}

/// 为请求固定实际发出时的宿主与会话身份。
pub(super) struct AdminSessionInvalidation {
    review: AdminSessionWriteReview,
    session: AdminSession,
}

impl AdminSessionInvalidation {
    /// 实发代际可核对时才登记；回执到达时不能重新认领宿主或代际。
    pub(super) fn capture() -> Option<Self> {
        let runtime = Runtime::try_current()?;
        runtime.try_current_scope_id()?;
        let review = try_consume_context::<AdminSessionWriteReview>()?;
        let session = try_consume_context::<AdminSession>()?;
        if !review.is_current()
            || admin_session_cookie().flatten().as_deref() != Some(session.csrf_token.as_str())
        {
            return None;
        }
        Some(Self { review, session })
    }

    /// 将可信的失效证据交给原宿主；状态机仍须核对当时的归属。
    pub(super) fn report(self) {
        self.review.invalidate.call((self.review.epoch, self.session));
    }
}

impl AdminSessionWriteReview {
    /// 旧认证宿主即使尚未完成卸载，也不能把回执交给新宿主。
    pub(super) fn is_current(self) -> bool {
        self.current_epoch.try_read().is_ok_and(|epoch| *epoch == self.epoch)
    }
}

/// API 与页面命令宿主共用门禁；缺少认证上下文或已卸载时失败关闭。
pub(crate) fn admin_session_writes_allowed() -> bool {
    let Some(runtime) = Runtime::try_current() else { return false };
    if runtime.try_current_scope_id().is_none() {
        return false;
    }
    try_consume_context::<AdminSessionWritePermission>().is_some_and(|permission| (permission.0)())
}

/// 写前复核没有可信结论时请求普通重查。
pub(super) fn request_admin_session_check() {
    let Some(runtime) = Runtime::try_current() else { return };
    if runtime.try_current_scope_id().is_none() {
        return;
    }
    if let Some(AdminSessionCheckTrigger(recheck)) = try_consume_context() {
        recheck.call(());
    }
}

/// 已认证 Admin 会话的浏览器状态；`authenticated = false` 不会构造该类型。
/// 调试格式保留会话元数据，但不能输出 CSRF token。
#[derive(Clone, PartialEq)]
pub(crate) struct AdminSession {
    pub actor: String,
    pub csrf_token: String,
    pub expires_at: Timestamp,
    pub absolute_expires_at: Timestamp,
    pub server_time: Timestamp,
    pub received_at_millis: f64,
}

/// 续期拒绝与服务端建议的最短等待。
pub(crate) struct AdminSessionRenewalFailure {
    pub(crate) failure: AdminApiFailure,
    pub(crate) retry_after_millis: Option<u64>,
}

impl AdminSession {
    /// actor 可以不变，轮换后的 CSRF 仍代表一个新的会话宿主。
    pub(crate) fn same_identity(&self, other: &Self) -> bool {
        self.actor == other.actor && self.csrf_token == other.csrf_token
    }

    /// 上报一次人工操作。写守卫核对 Cookie 与本宿主的 CSRF，避免替别的登录身份续期。
    pub(crate) async fn renew(&self) -> Result<Self, AdminSessionRenewalFailure> {
        let client_failure = |code: String| AdminSessionRenewalFailure {
            failure: AdminApiFailure::client(code),
            retry_after_millis: None,
        };
        let response = send_request(|abort_signal| {
            Request::post("/api/v1/admin/session/activity")
                .abort_signal(abort_signal)
                .header("x-csrf-token", &self.csrf_token)
                .build()
                .map_err(super::invalid_request_error)
        })
        .await
        .map_err(client_failure)?;
        if !response.ok() {
            let headers = response.headers();
            let retry_after_millis = retry_after_millis(
                headers.get("retry-after").as_deref(),
                headers.get("date").as_deref(),
            );
            let failure = read_response_failure(&response).await;
            return Err(AdminSessionRenewalFailure { failure, retry_after_millis });
        }
        let renewed = decode_session_response(response).await.map_err(client_failure)?;
        if renewed.actor != self.actor {
            return Err(client_failure("invalid_response".to_owned()));
        }
        Ok(renewed)
    }
}

fn retry_after_millis(retry_after: Option<&str>, server_date: Option<&str>) -> Option<u64> {
    let retry_after = retry_after?.trim();
    if !retry_after.is_empty() && retry_after.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(retry_after.parse::<u64>().unwrap_or(u64::MAX).saturating_mul(1_000));
    }
    // 绝对日期以同一响应的 Date 为基准，避免本机墙钟偏差缩短限流等待。
    let deadline = Timestamp::parse_from_imf_fixdate(retry_after)?;
    let sent = Timestamp::parse_from_imf_fixdate(server_date?)?;
    u64::try_from(deadline.signed_duration_since(sent).num_milliseconds().max(0)).ok()
}

impl std::fmt::Debug for AdminSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminSession")
            .field("actor", &self.actor)
            .field("csrf_token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .field("absolute_expires_at", &self.absolute_expires_at)
            .field("server_time", &self.server_time)
            .finish()
    }
}

/// 登录、读取与续期都依赖带服务端时钟的严格投影。
fn decode_admin_session(body: &str) -> Result<AdminSession, String> {
    let payload: ApiResponse<AdminSessionResponse> =
        serde_json::from_str(body).map_err(invalid_response_error)?;
    let data = payload.data;
    if payload.code != "ok"
        || !data.authenticated
        || data.actor.is_empty()
        || data.csrf_token.is_empty()
    {
        return Err("invalid_response".to_owned());
    }
    let Some(server_time) = data.server_time.as_deref() else {
        return Err("invalid_response".to_owned());
    };
    let parse = |value: &str| Timestamp::parse_from_rfc3339(value).map_err(invalid_response_error);
    let expires_at = parse(&data.expires_at)?;
    let absolute_expires_at = parse(&data.absolute_expires_at)?;
    let server_time = parse(server_time)?;
    if expires_at <= server_time || expires_at > absolute_expires_at {
        return Err("invalid_response".to_owned());
    }
    Ok(AdminSession {
        actor: data.actor,
        csrf_token: data.csrf_token,
        expires_at,
        absolute_expires_at,
        server_time,
        received_at_millis: monotonic_millis(),
    })
}

async fn decode_session_response(response: AdminResponse) -> Result<AdminSession, String> {
    let body = response.text().await.map_err(invalid_response_error)?;
    decode_admin_session(&body)
}

fn session_read_failure(failure: AdminApiFailure) -> Result<Option<AdminSession>, String> {
    // 只有完整回执能确认会话结束。复核失败暂停写入，保留原命令供会话重新确认后原样重试。
    if failure.status == Some(401) && failure.code == "unauthenticated" {
        Ok(None)
    } else {
        Err(failure.code)
    }
}

/// 只读校验当前 Admin 会话；由可信的未认证响应确认会话结束。
pub(crate) async fn fetch_admin_session() -> Result<Option<AdminSession>, String> {
    let response = send_get(SESSION_WITH_SERVER_TIME).await?;
    if !response.ok() {
        // 外壳自己的读票没有业务宿主归属；写前 GET 的可信 401 则按实发归属交回外壳。
        return session_read_failure(response_failure(response).await);
    }
    decode_session_response(response).await.map(Some)
}

/// 使用一次 Basic Auth 登录 Admin；服务端负责设置 HttpOnly session Cookie。
pub(crate) async fn login_admin(actor: String, secret: String) -> Result<AdminSession, String> {
    let credentials = format!("{actor}:{secret}");
    let authorization = format!("Basic {}", STANDARD.encode(credentials.as_bytes()));
    let response = send_request(|abort_signal| {
        Request::post(SESSION_WITH_SERVER_TIME)
            .abort_signal(abort_signal)
            .header("authorization", &authorization)
            .build()
            .map_err(super::invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    decode_session_response(response).await
}

/// 使用当前 CSRF token 注销 Admin，会话 Cookie 由服务端清除。
pub(crate) async fn logout_admin(csrf_token: String) -> Result<(), AdminApiFailure> {
    let response = send_request(|abort_signal| {
        Request::delete("/api/v1/admin/session")
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .build()
            .map_err(super::invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(read_response_failure(&response).await);
    }
    let payload: ApiResponse<Value> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    if payload.code != "ok" || !payload.data.is_null() {
        return Err(AdminApiFailure::client("invalid_response"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use auction_time::{Duration, Timestamp};
    use serde_json::{Value, json};

    use super::{AdminApiFailure, decode_admin_session, retry_after_millis, session_read_failure};

    const SESSION_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../auction-contracts/tests/fixtures/admin/session.json"
    ));

    #[test]
    fn only_a_complete_utf8_401_with_unauthenticated_confirms_session_end() {
        let valid =
            br#"{"code":"unauthenticated","message":"expired","data":null,"request_id":null}"#;
        let bad_utf8 = b"{\"code\":\"unauthenticated\",\"message\":\"\xff\",\"data\":null,\"request_id\":null}";
        let incomplete = br#"{"code":"unauthenticated","message":"expired","data":null}"#;
        let wrong_code =
            br#"{"code":"unknown_code","message":"expired","data":null,"request_id":null}"#;
        let failed_read =
            |status, body| session_read_failure(AdminApiFailure::from_http_bytes(status, body));
        assert_eq!(failed_read(401, valid), Ok(None));
        assert_eq!(failed_read(401, bad_utf8), Err("http_401".to_owned()));
        assert_eq!(failed_read(401, incomplete), Err("http_401".to_owned()));
        assert_eq!(failed_read(401, wrong_code), Err("unknown_code".to_owned()));
        assert_eq!(failed_read(503, valid), Err("http_503".to_owned()));
    }

    #[test]
    fn decodes_server_session_projection_with_server_time() {
        let server_time = Timestamp::parse_from_rfc3339("2026-11-18T01:02:03.456789Z").unwrap();
        let session = decode_admin_session(SESSION_FIXTURE).unwrap();
        assert_eq!(session.server_time, server_time);
        assert_eq!(
            session.expires_at.signed_duration_since(session.server_time),
            Duration::minutes(15)
        );
    }

    #[test]
    fn session_body_only_provides_identity_when_authenticated_with_actor_and_token() {
        let original = decode_admin_session(SESSION_FIXTURE).unwrap();
        let body: Value = serde_json::from_str(SESSION_FIXTURE).unwrap();
        let mut rotated = body.clone();
        rotated["data"]["csrf_token"] = json!("rotated-cookie");
        let rotated = decode_admin_session(&rotated.to_string()).unwrap();
        assert_eq!(rotated.actor, original.actor);
        assert!(!original.same_identity(&rotated));
        for (field, value) in
            [("authenticated", json!(false)), ("actor", json!("")), ("csrf_token", json!(""))]
        {
            let mut invalid = body.clone();
            invalid["data"][field] = value;
            assert_eq!(
                decode_admin_session(&invalid.to_string()).unwrap_err(),
                "invalid_response",
                "{field}"
            );
        }
    }

    #[test]
    fn timed_session_projection_rejects_missing_or_invalid_time_and_unknown_fields() {
        let body: Value = serde_json::from_str(SESSION_FIXTURE).unwrap();
        for value in [Value::Null, json!(42), json!("invalid")] {
            let mut wrong_time = body.clone();
            wrong_time["data"]["server_time"] = value;
            assert_eq!(
                decode_admin_session(&wrong_time.to_string()).unwrap_err(),
                "invalid_response"
            );
        }
        let mut missing_field = body.clone();
        missing_field["data"].as_object_mut().unwrap().remove("server_time");
        assert_eq!(
            decode_admin_session(&missing_field.to_string()).unwrap_err(),
            "invalid_response"
        );
        let mut unknown_field = body;
        unknown_field["data"]["extra"] = json!(true);
        assert_eq!(
            decode_admin_session(&unknown_field.to_string()).unwrap_err(),
            "invalid_response"
        );
    }

    #[test]
    fn retry_after_uses_delay_seconds_or_the_response_date() {
        assert_eq!(retry_after_millis(Some("12"), None), Some(12_000));
        let deadline = "Sun, 06 Nov 1994 08:49:37 GMT";
        let sent = "Sun, 06 Nov 1994 08:49:07 GMT";
        assert_eq!(retry_after_millis(Some(deadline), Some(sent)), Some(30_000));
        assert_eq!(retry_after_millis(Some(deadline), None), None);
        for value in [None, Some(""), Some("-1"), Some("1, 2")] {
            assert_eq!(retry_after_millis(value, Some(sent)), None);
        }
    }

    #[test]
    fn default_admin_session_fixture_keeps_the_published_fields() {
        use auction_contracts::{AdminSessionResponse, ApiResponse};

        let body =
            include_str!("../../../auction-contracts/tests/fixtures/admin/session-default.json");
        let legacy: ApiResponse<AdminSessionResponse> =
            serde_json::from_str(body).expect("缺省会话");
        assert!(legacy.data.server_time.is_none());
        assert!(decode_admin_session(body).is_err());
    }
    #[test]
    fn session_activity_contract_fixture() {
        use crate::api::contract_fixtures::admin_fixture;

        let body = admin_fixture!("session-activity");
        let session = super::decode_admin_session(body).expect("活动续期响应");
        assert_eq!(session.actor, "operator");
        assert!(session.expires_at > session.server_time);

        let mut invalid: serde_json::Value = serde_json::from_str(body).expect("续期样本");
        invalid["data"].as_object_mut().unwrap().remove("server_time").expect("服务端时刻");
        assert!(super::decode_admin_session(&invalid.to_string()).is_err());
    }
}
