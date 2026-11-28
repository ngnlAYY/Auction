//! Admin 会话登录、读取、人工操作续期和注销处理器。

use std::net::SocketAddr;

use auction_application::public::AdminSessionIdentity;
use auction_contracts::{AdminSessionResponse, ApiResponse};
use auction_time::Timestamp;
use axum::Json;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

use super::{ADMIN_CSRF_COOKIE, ADMIN_SESSION_COOKIE};
use crate::errors::{api_error, invalid_query};
use crate::mapping::timestamp;
use crate::query::parse_query;
use crate::session::{
    admin_cookie, admin_session_guard, append_clear_admin_cookies, authenticated_admin_mutation,
    authenticated_admin_session, authentication_error, internal_error, is_unauthenticated,
    map_admin_session_error, no_store_headers, read_admin_basic_auth, read_admin_cookie,
    session_unavailable,
};
use crate::state::AdminApiState;

pub(crate) async fn login_admin_session(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, true, true) {
        return response;
    }
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let Some((actor, secret)) = read_admin_basic_auth(&headers) else {
        state.security().login_failed();
        return api_error(StatusCode::UNAUTHORIZED, "invalid_credentials", "登录凭据无效")
            .into_response();
    };
    // 先确定响应投影，再创建会话和轮换 Cookie。
    let include_server_time = match session_projection(&uri) {
        Ok(include) => include,
        Err(response) => return *response,
    };
    let now = Timestamp::now();
    match session.login(actor, secret, now).await {
        Ok(result) => {
            state.security().login_succeeded();
            let mut response_headers = no_store_headers();
            let Ok(session_cookie) = admin_cookie(
                ADMIN_SESSION_COOKIE,
                &result.token,
                result.identity.absolute_expires_at,
                now,
                state.security().cookie_secure(),
                true,
            ) else {
                return internal_error();
            };
            let Ok(csrf_cookie) = admin_cookie(
                ADMIN_CSRF_COOKIE,
                &result.csrf_token,
                result.identity.absolute_expires_at,
                now,
                state.security().cookie_secure(),
                false,
            ) else {
                return internal_error();
            };
            response_headers.append(header::SET_COOKIE, session_cookie);
            response_headers.append(header::SET_COOKIE, csrf_cookie);
            session_response(
                "登录成功",
                response_headers,
                result.csrf_token,
                result.identity,
                include_server_time.then(Timestamp::now),
            )
        }
        Err(error) => {
            let response = map_admin_session_error(error);
            if response.status() == StatusCode::UNAUTHORIZED {
                state.security().login_failed();
            }
            response
        }
    }
}

pub(crate) async fn current_admin_session(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    let (raw_csrf, identity) = match authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let include_server_time = match session_projection(&uri) {
        Ok(include) => include,
        Err(response) => return *response,
    };
    session_response(
        "会话有效",
        no_store_headers(),
        raw_csrf,
        identity,
        include_server_time.then(Timestamp::now),
    )
}

// 已发布的缺省投影须保持字段闭集，扩展只在调用方显式请求时输出。
fn session_projection(uri: &Uri) -> Result<bool, Box<Response>> {
    let mut query =
        parse_query(uri, &["include"]).map_err(|()| Box::new(invalid_query("查询参数无效")))?;
    match query.remove("include").as_deref() {
        None => Ok(false),
        Some("server_time") => Ok(true),
        Some(_) => Err(Box::new(invalid_query("附加字段无效"))),
    }
}

pub(crate) async fn renew_admin_session(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    // 契约要求空请求体：零字节预算读取，任何非空正文都在认证与续期之前失败关闭，
    // 不缓冲未知大小正文，也不让被拒请求顺带延长空闲期限。
    if axum::body::to_bytes(body, 0).await.is_err() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_body", "会话续期只接受空请求体")
            .into_response();
    }
    let (raw_csrf, identity) =
        match authenticated_admin_mutation(&state, &parts.headers, address).await {
            Ok(result) => result,
            Err(response) => return *response,
        };
    session_response("会话已续期", no_store_headers(), raw_csrf, identity, Some(Timestamp::now()))
}

fn session_response(
    message: &str,
    headers: HeaderMap,
    csrf_token: String,
    identity: AdminSessionIdentity,
    server_time: Option<Timestamp>,
) -> Response {
    let data = AdminSessionResponse {
        authenticated: true,
        actor: identity.actor,
        csrf_token,
        expires_at: timestamp(identity.expires_at),
        absolute_expires_at: timestamp(identity.absolute_expires_at),
        server_time: server_time.map(timestamp),
    };
    (StatusCode::OK, headers, Json(ApiResponse::ok(message, data))).into_response()
}

pub(crate) async fn logout_admin_session(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = authenticated_admin_mutation(&state, &headers, address).await {
        return *response;
    }
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let Some(raw_token) = read_admin_cookie(&headers, ADMIN_SESSION_COOKIE) else {
        return authentication_error();
    };
    let now = Timestamp::now();
    if let Err(error) = session.logout(raw_token, now).await {
        if is_unauthenticated(&error) {
            return authentication_error();
        }
        return session_unavailable();
    }
    let mut response_headers = no_store_headers();
    append_clear_admin_cookies(&mut response_headers, state.security().cookie_secure());
    (StatusCode::OK, response_headers, Json(ApiResponse::ok("会话已注销", Option::<()>::None)))
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};
    use std::time::Duration as StdDuration;

    use auction_application::admin::{AdminWorldPort, AdminWorldSetEnabledCommand};
    use auction_application::auction::WorldSnapshot;
    use auction_application::core::{ApplicationFuture, SessionError};
    use auction_application::public::{AdminLoginResult, AdminSessionIdentity, AdminSessionPort};
    use auction_contracts::{AdminSessionResponse, ApiErrorResponse, ApiResponse};
    use auction_time::{Duration, Timestamp};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::extract::ConnectInfo;
    use axum::http::{Method, Request, StatusCode, header};
    use axum::response::Response;
    use rust_embed::{EmbeddedFile, RustEmbed};
    use serde::de::DeserializeOwned;
    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use tower::ServiceExt;

    use super::session_response;
    use crate::admin::{ADMIN_CSRF_COOKIE, ADMIN_CSRF_HEADER, ADMIN_SESSION_COOKIE};
    use crate::mapping::timestamp;
    use crate::middleware::security::{AdminHttpSecurity, HttpSecurityOptions};
    use crate::server::admin_router;
    use crate::session::no_store_headers;
    use crate::state::AdminApiState;

    const SESSION: &str = "/api/v1/admin/session";
    const SESSION_WITH_SERVER_TIME: &str = "/api/v1/admin/session?include=server_time";
    const ACTIVITY: &str = "/api/v1/admin/session/activity";
    const ORIGIN: &str = "https://admin.example.test";
    const TOKEN: &str = "test-session";
    const CSRF: &str = "test-csrf";
    const SESSION_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../auction-contracts/tests/fixtures/admin/session.json"
    ));

    struct EmptyAssets;

    impl RustEmbed for EmptyAssets {
        fn get(_file_path: &str) -> Option<EmbeddedFile> {
            None
        }

        fn iter() -> impl Iterator<Item = Cow<'static, str>> + 'static {
            std::iter::empty()
        }
    }

    struct TestSessions(Mutex<AdminSessionIdentity>);

    fn session_identity(now: Timestamp, idle_ttl: Duration) -> AdminSessionIdentity {
        AdminSessionIdentity {
            actor: "operator".to_owned(),
            expires_at: now.checked_add_signed(idle_ttl).unwrap(),
            absolute_expires_at: now.checked_add_signed(Duration::hours(8)).unwrap(),
        }
    }

    // 旧页面的严格解码闭集，与当前 DTO 独立，避免新增字段悄悄改变断言。
    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct PublishedSession {
        authenticated: bool,
        actor: String,
        csrf_token: String,
        expires_at: String,
        absolute_expires_at: String,
    }

    impl AdminSessionPort for TestSessions {
        fn login(
            &self,
            _actor: String,
            _secret: Vec<u8>,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, AdminLoginResult> {
            Box::pin(async move {
                Ok(AdminLoginResult {
                    token: TOKEN.to_owned(),
                    csrf_token: CSRF.to_owned(),
                    identity: self.0.lock().unwrap().clone(),
                })
            })
        }

        fn current(
            &self,
            token: String,
            csrf: String,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, AdminSessionIdentity> {
            Box::pin(async move {
                if token != TOKEN || csrf != CSRF {
                    return Err(SessionError::Unauthenticated.into());
                }
                Ok(self.0.lock().unwrap().clone())
            })
        }

        fn touch(
            &self,
            token: String,
            csrf: String,
            now: Timestamp,
        ) -> ApplicationFuture<'_, AdminSessionIdentity> {
            Box::pin(async move {
                self.current(token, csrf, now).await?;
                let mut identity = self.0.lock().unwrap();
                identity.expires_at = now.checked_add_signed(Duration::minutes(15)).unwrap();
                Ok(identity.clone())
            })
        }

        fn logout(&self, _token: String, _now: Timestamp) -> ApplicationFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    struct EmptyWorlds;

    impl AdminWorldPort for EmptyWorlds {
        fn list_worlds(&self) -> ApplicationFuture<'_, Vec<WorldSnapshot>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn set_enabled(
            &self,
            _command: AdminWorldSetEnabledCommand,
        ) -> ApplicationFuture<'_, WorldSnapshot> {
            unreachable!("测试只读取列表");
        }
    }

    fn test_router(sessions: Arc<TestSessions>) -> Router {
        let security = AdminHttpSecurity::new(HttpSecurityOptions {
            cookie_secure: true,
            origins_enabled: true,
            allowed_origins: &[ORIGIN.to_owned()],
            login_limit: 4,
            mutation_limit: 3,
            anonymous_read_limit: 1,
            anonymous_read_concurrency: 1,
            rate_limit_window: StdDuration::from_secs(60),
            request_timeout: StdDuration::from_secs(15),
            trusted_proxy_cidrs: &[],
            trusted_unix_proxy: false,
        })
        .unwrap();
        let state = AdminApiState::new(
            Some(sessions),
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
        )
        .with_admin_worlds(Arc::new(EmptyWorlds));
        admin_router::<EmptyAssets>(env!("CARGO_PKG_VERSION"), state.http_boundary())
            .with_state(state)
    }

    async fn request(
        router: &Router,
        method: Method,
        path: &str,
        csrf_header: Option<&str>,
    ) -> Response {
        let mut request = Request::builder()
            .method(method.clone())
            .uri(path)
            .header(
                header::COOKIE,
                format!("{ADMIN_SESSION_COOKIE}={TOKEN}; {ADMIN_CSRF_COOKIE}={CSRF}"),
            )
            .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        if method == Method::POST {
            request = request.header(header::ORIGIN, ORIGIN);
            if path.split('?').next() == Some(SESSION) {
                request = request.header(header::AUTHORIZATION, "Basic b3BlcmF0b3I6dGVzdA==");
            }
        }
        if let Some(csrf) = csrf_header {
            request = request.header(ADMIN_CSRF_HEADER, csrf);
        }
        router.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap()
    }

    async fn session_data<T: DeserializeOwned>(response: Response) -> T {
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        serde_json::from_slice::<ApiResponse<T>>(&body).unwrap().data
    }

    async fn session_body(response: Response) -> AdminSessionResponse {
        session_data(response).await
    }

    fn assert_server_time(
        server_time: &str,
        expires_at: &str,
        sent: Timestamp,
        received: Timestamp,
    ) {
        let time = Timestamp::parse_from_rfc3339(server_time).unwrap();
        assert_eq!(server_time, timestamp(time));
        assert!(
            (sent.timestamp_micros()..=received.timestamp_micros())
                .contains(&time.timestamp_micros())
        );
        assert!(time < Timestamp::parse_from_rfc3339(expires_at).unwrap());
    }

    #[tokio::test]
    async fn published_admin_session_routes_add_server_time_only_on_request() {
        let sessions = Arc::new(TestSessions(Mutex::new(session_identity(
            Timestamp::now(),
            Duration::minutes(15),
        ))));
        let router = test_router(sessions);
        for method in [Method::GET, Method::POST] {
            let published: PublishedSession =
                session_data(request(&router, method.clone(), SESSION, None).await).await;
            assert!(published.authenticated);
            let sent = Timestamp::now();
            let response = request(&router, method, SESSION_WITH_SERVER_TIME, None).await;
            let received = Timestamp::now();
            let mut extended: Value = session_data(response).await;
            let server_time = extended.as_object_mut().unwrap().remove("server_time").unwrap();
            assert_server_time(
                server_time.as_str().unwrap(),
                &published.expires_at,
                sent,
                received,
            );
            assert_eq!(extended, serde_json::to_value(published).unwrap());
        }
    }

    #[tokio::test]
    async fn published_admin_session_routes_reject_invalid_projections() {
        let sessions = Arc::new(TestSessions(Mutex::new(session_identity(
            Timestamp::now(),
            Duration::minutes(15),
        ))));
        let router = test_router(sessions);
        for method in [Method::GET, Method::POST] {
            for query in [
                "include=other",
                "include=",
                "include=server_time&include=server_time",
                "fields=server_time",
            ] {
                let path = format!("{SESSION}?{query}");
                let response = request(&router, method.clone(), &path, None).await;
                assert!(!response.headers().contains_key(header::SET_COOKIE));
                assert_error(response, StatusCode::BAD_REQUEST, "invalid_query").await;
            }
        }
    }

    #[tokio::test]
    async fn timed_admin_session_response_matches_shared_fixture() {
        let server_time = Timestamp::parse_from_rfc3339("2026-11-18T01:02:03.456789Z").unwrap();
        let response = session_response(
            "会话有效",
            no_store_headers(),
            "fixture-csrf-placeholder".to_owned(),
            session_identity(server_time, Duration::minutes(15)),
            Some(server_time),
        );
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let expected: Value = serde_json::from_str(SESSION_FIXTURE).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), expected);
        let baseline = session_response(
            "会话有效",
            no_store_headers(),
            "fixture-csrf-placeholder".to_owned(),
            session_identity(server_time, Duration::minutes(15)),
            None,
        );
        crate::server::contract_fixtures::assert_http(
            "admin/session-default",
            StatusCode::OK,
            baseline,
        )
        .await;
    }

    #[tokio::test]
    async fn admin_session_activity_contract_fixtures() {
        use crate::server::contract_fixtures::{assert_wire, response_json, wire_fixture};

        let sent = Timestamp::now();
        let sessions =
            Arc::new(TestSessions(Mutex::new(session_identity(sent, Duration::minutes(1)))));
        let router = test_router(sessions.clone());
        let response = request(&router, Method::POST, ACTIVITY, Some(CSRF)).await;
        let received = Timestamp::now();
        let body = response_json(StatusCode::OK, response).await;
        let data = &body["data"];
        assert_server_time(
            data["server_time"].as_str().unwrap(),
            data["expires_at"].as_str().unwrap(),
            sent,
            received,
        );
        let identity = sessions.0.lock().unwrap().clone();
        assert_eq!(data["expires_at"], timestamp(identity.expires_at));
        assert_eq!(data["absolute_expires_at"], timestamp(identity.absolute_expires_at));

        // 用同一个生产构造器固定已在实际续期请求上核对的三个墙钟字段。
        let at = Timestamp::parse_from_rfc3339("2026-11-18T01:02:03.456789Z").unwrap();
        let fixed = response_json(
            StatusCode::OK,
            session_response(
                "会话已续期",
                no_store_headers(),
                CSRF.to_owned(),
                session_identity(at, Duration::minutes(15)),
                Some(at),
            ),
        )
        .await;
        assert_wire("admin/session-activity", &fixed);
        let mut expected = wire_fixture("admin/session-activity");
        for key in ["server_time", "expires_at", "absolute_expires_at"] {
            expected["data"][key] = data[key].clone();
        }
        assert_eq!(body, expected);
    }

    async fn assert_error(response: Response, status: StatusCode, code: &str) {
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(serde_json::from_slice::<ApiErrorResponse>(&body).unwrap().code, code);
    }

    #[tokio::test]
    async fn admin_session_reads_do_not_renew_and_activity_uses_write_guards() {
        let identity = session_identity(Timestamp::now(), Duration::minutes(1));
        let expires_at = identity.expires_at;
        let sessions = Arc::new(TestSessions(Mutex::new(identity)));
        let router = test_router(sessions.clone());
        let login = session_body(request(&router, Method::POST, SESSION, None).await).await;
        assert_eq!(login.expires_at, timestamp(expires_at));
        let session = session_body(request(&router, Method::GET, SESSION, None).await).await;
        assert_eq!(session.expires_at, login.expires_at);
        let worlds = request(&router, Method::GET, "/api/v1/admin/worlds", None).await;
        assert_eq!(worlds.status(), StatusCode::OK);
        assert_eq!(sessions.0.lock().unwrap().expires_at, expires_at);
        for csrf in [None, Some("wrong-csrf")] {
            let failed = request(&router, Method::POST, ACTIVITY, csrf).await;
            assert_error(failed, StatusCode::FORBIDDEN, "csrf_invalid").await;
        }
        for method in [Method::GET, Method::HEAD] {
            let failed = request(&router, method, ACTIVITY, Some(CSRF)).await;
            assert_eq!(failed.status(), StatusCode::METHOD_NOT_ALLOWED);
            assert_eq!(failed.headers()[header::ALLOW], "POST");
        }
        assert_eq!(sessions.0.lock().unwrap().expires_at, expires_at);
        let sent = Timestamp::now();
        let response = request(&router, Method::POST, ACTIVITY, Some(CSRF)).await;
        let received = Timestamp::now();
        let refreshed = session_body(response).await;
        assert_server_time(
            refreshed.server_time.as_deref().unwrap(),
            &refreshed.expires_at,
            sent,
            received,
        );
        assert_eq!(refreshed.expires_at, timestamp(sessions.0.lock().unwrap().expires_at));
        assert_ne!(refreshed.expires_at, session.expires_at);
        assert_eq!(refreshed.absolute_expires_at, session.absolute_expires_at);

        let limited = request(&router, Method::POST, ACTIVITY, Some(CSRF)).await;
        assert!(limited.headers().contains_key(header::RETRY_AFTER));
        assert_error(limited, StatusCode::TOO_MANY_REQUESTS, "rate_limited").await;
        let checked = session_body(request(&router, Method::GET, SESSION, None).await).await;
        assert_eq!(checked.expires_at, refreshed.expires_at);
    }
}
