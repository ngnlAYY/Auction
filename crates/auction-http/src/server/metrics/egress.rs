//! 只挂载指标路由的 HTTP 适配；任务取消所有权仍由调用方的 `TaskGuard` 持有。

use std::{future::Future, io, net::SocketAddr, panic::AssertUnwindSafe, sync::Arc};

use auction_application::runtime_switches::RuntimeSwitches;
use auction_observability::{METRICS_ROUTE, ProcessMetrics};
use axum::{
    Router,
    extract::State,
    http::{Method, StatusCode, Uri},
    middleware,
    response::Response,
};
use futures_util::FutureExt;
use tokio::net::TcpListener;

use crate::middleware::boundary;
use crate::server;

/// 绑定并运行本角色的附加指标出口；调用方负责持有返回任务的取消所有权。
/// 绑定失败只降级采集，不参与业务监督或 readiness。
pub fn spawn_metrics_listener(
    address: SocketAddr,
    role: &'static str,
    metrics: Arc<ProcessMetrics>,
    switches: RuntimeSwitches,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let listener = match TcpListener::bind(address).await {
            Ok(listener) => listener,
            Err(error) => {
                tracing::error!(
                    kind = "metrics_bind_failed", role, io_kind = ?error.kind(),
                    "附加指标绑定失败；仅降级采集"
                );
                return;
            }
        };
        tracing::info!(role, %address, "附加指标出口已监听");
        let result = serve_metrics(listener, metrics, switches, std::future::pending()).await;
        match result {
            Ok(()) => tracing::error!(kind = "metrics_serve_stopped", role, "附加指标服务意外停止"),
            Err(error) => tracing::error!(
                kind = "metrics_serve_failed", role, io_kind = ?error.kind(),
                "附加指标服务失败；仅降级采集"
            ),
        }
    })
}

/// 运行已绑定的附加出口，复用业务入口的边界及指标渲染。
/// 只持有注册表，不装配业务、健康、控制或静态资源路由。
pub async fn serve_metrics(
    listener: TcpListener,
    metrics: Arc<ProcessMetrics>,
    switches: auction_application::runtime_switches::RuntimeSwitches,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    let boundary = boundary::HttpBoundary::default()
        .with_runtime_switches(switches)
        .with_metrics(metrics.http());
    let app = Router::new()
        .route(METRICS_ROUTE, axum::routing::any(metrics_response))
        .fallback(|| async { StatusCode::NOT_FOUND })
        .with_state(metrics)
        .layer(middleware::from_fn_with_state(boundary, boundary::enforce));
    match AssertUnwindSafe(server::serve_http(listener, app, shutdown)).catch_unwind().await {
        Ok(result) => result,
        Err(panic) => {
            // 不输出 panic payload；装配根按固定类别报告并降级采集。
            drop(panic);
            Err(io::Error::other("附加指标服务异常退出"))
        }
    }
}

async fn metrics_response(
    State(metrics): State<Arc<ProcessMetrics>>,
    method: Method,
    uri: Uri,
) -> Response {
    super::render(Some(&metrics), method, uri)
}
