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

pub use metrics::{serve_metrics, spawn_metrics_listener};
pub use readiness::HttpReadinessProbe;
pub use surfaces::{admin_router, public_router};
#[cfg(target_os = "linux")]
pub use unix_listener::{UnixHttpListener, UnixListenerOptions};

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics;

use std::{future::Future, io, net::SocketAddr, time::Duration};

use axum::{Router, body::Body, extract::ConnectInfo, serve::Listener};
use hyper::body::Incoming;
use hyper_util::{
    rt::{TokioExecutor, TokioIo, TokioTimer},
    server::{conn::auto::Builder, graceful::GracefulShutdown},
    service::TowerToHyperService,
};
use tokio::task::JoinSet;
use tower::ServiceExt;

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
