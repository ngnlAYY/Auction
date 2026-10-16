//! Public/Admin 共用的 Hyper 连接服务。
//!
//! Axum 的便捷 `serve` 不暴露请求头读取超时。本模块保持 Axum Router 所有权不变，只在
//! 连接层固定 5 秒 HTTP/1 header timeout，并统一跟踪连接级优雅关闭。

use std::{future::Future, io, net::SocketAddr, time::Duration};

use axum::{Extension, Router, body::Body, extract::ConnectInfo, serve::Listener};
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
///
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
                let mut app = app.clone();
                if let Some(address) = address {
                    app = app.layer(Extension(ConnectInfo(address)));
                }
                connections.spawn(async move {
                    let service = app
                        .map_request(|request: hyper::Request<Incoming>| request.map(Body::new));
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
                        tracing::debug!(?address, error = %error, "HTTP connection closed with error");
                    }
                });
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
        tracing::warn!(error = %error, "HTTP connection task join failed");
    }
}
