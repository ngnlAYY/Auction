//! Public/Admin 共用的 Hyper 连接服务。
//! Axum 的便捷 `serve` 不暴露请求头读取超时。本模块保持 Axum Router 所有权不变，只在
//! 连接层固定 5 秒 HTTP/1 header timeout，并统一跟踪连接级优雅关闭。

pub(crate) mod build_identity;
mod health;
pub(crate) mod metrics;
mod readiness;
mod surfaces;
#[cfg(target_os = "linux")]
mod unix_listener;

pub use metrics::spawn_metrics_listener;
pub use readiness::HttpReadinessProbe;
pub use surfaces::{admin_router, public_router};
#[cfg(target_os = "linux")]
pub use unix_listener::{UnixHttpListener, UnixListenerOptions};

use std::{future::Future, io, net::SocketAddr, time::Duration};

use auction_application::core::diagnostics::DiagnosticContext;
use axum::{Router, body::Body, extract::ConnectInfo, serve::Listener};
use hyper::body::Incoming;
use hyper_util::{
    rt::{TokioExecutor, TokioIo, TokioTimer},
    server::{conn::auto::Builder, graceful::GracefulShutdown},
    service::TowerToHyperService,
};
use tokio::task::JoinSet;
use tower::ServiceExt;

use crate::diagnostics;

const READ_HEADER_TIMEOUT: Duration = Duration::from_secs(5);

/// 运行一个已绑定的 TCP 或 Unix listener，并在停止接收后等待现有 Hyper 连接排空。
/// TCP listener 保留真实 `SocketAddr`；Unix listener 返回 `None`，不伪造 TCP peer。
/// 可信 Unix 代理策略再由 HTTP 边界决定是否接纳 `X-Forwarded-For`。
pub async fn serve_http<L, F>(mut listener: L, app: Router, shutdown: F) -> io::Result<()>
where
    L: Listener,
    L::Addr: Into<Option<SocketAddr>>,
    F: Future<Output = ()> + Send + 'static,
{
    let graceful = GracefulShutdown::new();
    let mut connections = JoinSet::new();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            (stream, address) = listener.accept() => {
                let watcher = graceful.watcher();
                let address = address.into();
                let app = app.clone();
                let body = async move {
                    // 连接地址直接写入请求，避免为每条 TCP 连接重建全部路由的 middleware。
                    let service = app.map_request(move |mut request: hyper::Request<Incoming>| {
                        if let Some(address) = address {
                            request.extensions_mut().insert(ConnectInfo(address));
                        }
                        request.map(Body::new)
                    });
                    let service = TowerToHyperService::new(service);
                    let mut builder = Builder::new(TokioExecutor::new());
                    builder
                        .http1()
                        .timer(TokioTimer::new())
                        .header_read_timeout(Some(READ_HEADER_TIMEOUT));
                    // HTTP/2 CONNECT 是 WebSocket 升级在支持该协议的代理链上所需的能力。
                    builder.http2().enable_connect_protocol();
                    let connection = builder
                        .serve_connection_with_upgrades(TokioIo::new(stream), service)
                        .into_owned();
                    if let Err(error) = watcher.watch(connection).await {
                        diagnostics::debug(
                            "http_transport",
                            "serve_connection",
                            diagnostics::connection_kind(&error),
                            DiagnosticContext {
                                route: Some("__connection__"),
                                ..Default::default()
                            },
                        );
                    }
                };
                connections.spawn(body);
            }
            result = connections.join_next(), if !connections.is_empty() => {
                log_connection_join(result);
            }
        }
    }

    graceful.shutdown().await;
    while let Some(result) = connections.join_next().await {
        log_connection_join(Some(result));
    }
    Ok(())
}

fn log_connection_join(result: Option<Result<(), tokio::task::JoinError>>) {
    if let Some(Err(error)) = result
        && !error.is_cancelled()
    {
        diagnostics::warn(
            "http_transport",
            "join_connection",
            diagnostics::join_kind(&error),
            DiagnosticContext { route: Some("__connection__"), ..Default::default() },
        );
    }
}

/// 已绑定的 Public/Admin HTTP listener；Unix 变体只在 Linux 目标编译。
/// 描述只包含监听地址或 socket 路径，不读取凭据或配置。
pub enum HttpListener {
    Tcp(tokio::net::TcpListener),
    #[cfg(target_os = "linux")]
    Unix(unix_listener::UnixHttpListener),
}

impl HttpListener {
    /// 返回不含敏感配置的监听描述，用于启动日志。
    pub fn description(&self) -> String {
        match self {
            Self::Tcp(listener) => listener
                .local_addr()
                .map_or_else(|_| "tcp".to_owned(), |address| address.to_string()),
            #[cfg(target_os = "linux")]
            Self::Unix(listener) => format!("unix:{}", listener.socket_path().display()),
        }
    }

    /// 使用统一的 `SocketAddr` connect-info 运行 TCP 或 Unix 路由。
    pub async fn serve<F>(self, app: Router, shutdown: F) -> io::Result<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        match self {
            Self::Tcp(listener) => serve_http(listener, app, shutdown).await,
            #[cfg(target_os = "linux")]
            Self::Unix(listener) => serve_http(listener, app, shutdown).await,
        }
    }
}

#[cfg(test)]
pub(crate) mod contract_fixtures {
    use std::borrow::Cow;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::sync::Arc;

    use auction_application::admin::{
        AdminAuditPort, AdminCredentialPort, AdminMailPort, AdminProxyEventReader, AdminRulesPort,
        AdminUserPort,
    };
    use auction_application::core::{ApplicationError, ApplicationFuture};
    use auction_application::delivery::DeliveryAdminPort;
    use auction_application::operations::OperationsAlertSourceReader;
    use auction_application::public::{AdminLoginResult, AdminSessionIdentity, AdminSessionPort};
    use auction_application::realtime::RealtimeAdminPort;
    use auction_application::settlement::SettlementReportReader;
    use auction_observability::HealthReader;
    use auction_time::{Duration, Timestamp};
    use axum::body::{Body, to_bytes};
    use axum::extract::ConnectInfo;
    use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
    use axum::response::Response;
    use serde::Serialize;
    use serde_json::Value;
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::admin_router;
    use crate::middleware::boundary::HttpBoundary;
    use crate::state::AdminApiState;

    /// 固定生产序列化的完整字段集合，供浏览器共用。
    pub(crate) fn assert_wire(name: &str, actual: impl Serialize) {
        let actual = serde_json::to_value(actual).expect("序列化响应");
        assert_eq!(actual, wire_fixture(name), "{name}");
    }

    pub(crate) fn wire_fixture(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../auction-contracts/tests/fixtures")
            .join(format!("{name}.json"));
        let body = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("读取契约样本 {name}：{error}"));
        serde_json::from_str(&body).expect("契约样本 JSON")
    }

    /// 有状态码的响应先过生产 HTTP 序列化。
    pub(crate) async fn assert_http(name: &str, status: StatusCode, response: Response) {
        assert_wire(name, response_json(status, response).await);
    }

    pub(crate) async fn response_json(status: StatusCode, response: Response) -> Value {
        let actual_status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("响应正文");
        let actual: Value = serde_json::from_slice(&body).expect("响应 JSON");
        assert_eq!(actual_status, status, "{actual}");
        actual
    }

    pub(crate) const AUCTION_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2b);

    pub(crate) fn time() -> Timestamp {
        Timestamp::from_timestamp(1_763_200_000, 110_000_000).expect("样本时刻")
    }

    pub(crate) fn admin_address() -> ConnectInfo<SocketAddr> {
        ConnectInfo(SocketAddr::from(([192, 0, 2, 21], 3000)))
    }

    pub(crate) fn admin_headers() -> HeaderMap {
        HeaderMap::from_iter([
            (
                header::COOKIE,
                HeaderValue::from_static(
                    "auction_admin_session=fixture-placeholder; auction_admin_csrf=fixture-placeholder",
                ),
            ),
            (
                header::HeaderName::from_static(crate::admin::ADMIN_CSRF_HEADER),
                HeaderValue::from_static("fixture-placeholder"),
            ),
        ])
    }

    pub(crate) struct FixtureAdminSession;

    impl AdminSessionPort for FixtureAdminSession {
        fn login(
            &self,
            _actor: String,
            _secret: Vec<u8>,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, AdminLoginResult> {
            Box::pin(async {
                Err(ApplicationError::CapabilityUnavailable("样本会话不登录"))
            })
        }

        fn current(
            &self,
            _raw_token: String,
            _raw_csrf: String,
            now: Timestamp,
        ) -> ApplicationFuture<'_, AdminSessionIdentity> {
            Box::pin(async move {
                Ok(AdminSessionIdentity {
                    actor: "fixture-admin".to_owned(),
                    expires_at: now + Duration::minutes(10),
                    absolute_expires_at: now + Duration::hours(1),
                })
            })
        }

        fn touch(
            &self,
            token: String,
            csrf: String,
            now: Timestamp,
        ) -> ApplicationFuture<'_, AdminSessionIdentity> {
            self.current(token, csrf, now)
        }

        fn logout(&self, _raw_token: String, _now: Timestamp) -> ApplicationFuture<'_, ()> {
            Box::pin(async {
                Err(ApplicationError::CapabilityUnavailable("样本会话不退出"))
            })
        }
    }

    pub(crate) fn admin_state() -> AdminApiState {
        AdminFixturePorts::default().state()
    }

    #[derive(Default)]
    pub(crate) struct AdminFixturePorts {
        pub(crate) audit: Option<Arc<dyn AdminAuditPort>>,
        pub(crate) health: Option<Arc<dyn HealthReader>>,
        pub(crate) mail: Option<Arc<dyn AdminMailPort>>,
        pub(crate) rules: Option<Arc<dyn AdminRulesPort>>,
        pub(crate) credentials: Option<Arc<dyn AdminCredentialPort>>,
        pub(crate) users: Option<Arc<dyn AdminUserPort>>,
        pub(crate) settlement_report: Option<Arc<dyn SettlementReportReader>>,
        pub(crate) proxy_events: Option<Arc<dyn AdminProxyEventReader>>,
        pub(crate) deliveries: Option<Arc<dyn DeliveryAdminPort>>,
        pub(crate) alert_source: Option<Arc<dyn OperationsAlertSourceReader>>,
        pub(crate) realtime: Option<Arc<dyn RealtimeAdminPort>>,
    }

    impl AdminFixturePorts {
        pub(crate) fn state(self) -> AdminApiState {
            AdminApiState::new(
                Some(Arc::new(FixtureAdminSession)),
                None,
                None,
                self.audit,
                self.health,
                None,
                self.mail,
                self.rules,
                None,
                self.credentials,
                None,
                self.users,
                None,
                self.settlement_report,
                None,
                None,
                None,
                None,
                self.proxy_events,
                self.deliveries,
                None,
                self.alert_source,
                None,
                self.realtime,
                None,
                crate::AdminHttpSecurity::default(),
            )
        }
    }

    struct NoAssets;

    impl rust_embed::RustEmbed for NoAssets {
        fn get(_path: &str) -> Option<rust_embed::EmbeddedFile> {
            None
        }

        fn iter() -> impl Iterator<Item = Cow<'static, str>> {
            std::iter::empty()
        }
    }

    /// 经真实 Admin 路由与生产 handler 处理请求，测试只替换应用端口。
    pub(crate) async fn admin_get(state: AdminApiState, path: &str) -> Response {
        let mut request = Request::get(path).body(Body::empty()).expect("Admin GET");
        *request.headers_mut() = admin_headers();
        request.extensions_mut().insert(admin_address());
        admin_router::<NoAssets>("fixture", HttpBoundary::default())
            .with_state(state)
            .oneshot(request)
            .await
            .expect("Admin 响应")
    }
}
