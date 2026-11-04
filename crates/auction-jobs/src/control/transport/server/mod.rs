//! Worker control Unix socket 服务端装配；服务生命周期、peer 凭据过滤和 socket
//! 文件安全由子模块拥有，不会启动第二个监听器。

use auction_application::diagnostics::DiagnosticContext;

use super::*;
use crate::worker::diagnostics;

#[cfg(unix)]
mod listener;
#[cfg(unix)]
mod security;

#[cfg(unix)]
use listener::PeerCheckedUnixListener;
#[cfg(unix)]
use security::{
    SocketCleanupGuard, acquire_instance_lock, prepare_socket_path, remove_if_same_socket,
    resolve_group_id, secure_bound_socket, socket_identity, validate_runtime_directory,
};

/// 只监听本地 Unix socket 的 Worker control server。
#[cfg(unix)]
pub struct WorkerControlServer {
    listener: PeerCheckedUnixListener,
    handle: WorkerControlHandle,
    config: WorkerControlServerConfig,
    metrics: Option<Arc<ProcessMetrics>>,
    /// W1.10 远端只读执行器；未装配时对应路由明确返回能力不可用。
    remote_reads: Option<Arc<dyn WorkerRemoteReadPort>>,
    // 字段按声明顺序析构；未进入 run 就 Drop 时也必须先清理 socket，最后释放实例锁。
    _socket_guard: SocketCleanupGuard,
    _lock: File,
}

#[cfg(unix)]
impl WorkerControlServer {
    /// 绑定 socket；已有目标只有在确认为 socket 文件时才会被清理。
    pub async fn bind(
        config: WorkerControlServerConfig,
        handle: WorkerControlHandle,
    ) -> Result<Self, WorkerControlServerError> {
        Self::bind_with_metrics(config, handle, None).await
    }

    /// 绑定 socket 并可选挂载进程本地 Prometheus registry。
    /// 指标路由复用同一个 peer-checked Unix listener；未注入或关闭指标时仍 fail closed 为
    /// 404，不会启动第二个 HTTP 监听器。
    pub async fn bind_with_metrics(
        config: WorkerControlServerConfig,
        handle: WorkerControlHandle,
        metrics: Option<Arc<ProcessMetrics>>,
    ) -> Result<Self, WorkerControlServerError> {
        let group_id = resolve_group_id(&config.group)?;
        validate_runtime_directory(&config.runtime_directory)?;
        let lock = acquire_instance_lock(&config.socket_path)?;
        prepare_socket_path(&config.socket_path).await?;
        let listener = tokio::net::UnixListener::bind(&config.socket_path)
            .map_err(WorkerControlServerError::Io)?;
        let identity = socket_identity(&config.socket_path)?;
        match secure_bound_socket(&config.socket_path, group_id, identity) {
            Ok(()) => {}
            Err(error) => {
                drop(listener);
                let _ = remove_if_same_socket(&config.socket_path, Some(identity));
                return Err(error);
            }
        }
        let allowed_peer_uids = config.allowed_peer_uids.iter().copied().collect();
        let allowed_peer_gids = config.allowed_peer_gids.iter().copied().collect();
        let cleanup_path = config.socket_path.clone();
        Ok(Self {
            listener: PeerCheckedUnixListener { listener, allowed_peer_uids, allowed_peer_gids },
            handle,
            config,
            metrics,
            remote_reads: None,
            _lock: lock,
            _socket_guard: SocketCleanupGuard::new(cleanup_path, identity),
        })
    }

    /// 注入 W1.10 远端只读执行器；Admin 装备读取与 CLI Sent 核查共用 Worker 的请求门、
    /// 账号熔断与 W1.9 暂停判定，Admin/CLI 不再持有第二个限速器。
    pub fn with_remote_reads(mut self, remote_reads: Arc<dyn WorkerRemoteReadPort>) -> Self {
        self.remote_reads = Some(remote_reads);
        self
    }

    /// 运行控制 server，收到 shutdown future 后优雅收口并清理 socket。
    pub async fn run(
        self,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<(), WorkerControlServerError> {
        let Self {
            listener,
            handle,
            config,
            metrics: process_metrics,
            remote_reads,
            _lock: lock,
            _socket_guard: socket_guard,
        } = self;
        let request_timeout = config.request_timeout;
        let metrics_state = process_metrics;
        let app = Router::new()
            .route(WORKER_CONTROL_LIVE_PATH, get(live))
            .route(WORKER_CONTROL_READY_PATH, get(ready))
            .route(WORKER_CONTROL_STATUS_PATH, get(status))
            .route(WORKER_CONTROL_MAIL_PROGRESS_PATH, get(mail_progress))
            .route(WORKER_CONTROL_TRIGGER_POLL_PATH, post(trigger_poll))
            .route(WORKER_CONTROL_EQUIPMENT_READ_PATH, post(equipment_read))
            .route(WORKER_CONTROL_SENT_LOOKUP_PATH, post(sent_lookup))
            .route(WORKER_CONTROL_METRICS_PATH, get(metrics))
            .fallback(not_found)
            .layer(DefaultBodyLimit::max(config.body_limit))
            .layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| async move {
                    // 远端读取跨越 Worker 自己的 fetch 预算，使用独立的更长上界。
                    let path = request.uri().path().to_owned();
                    let budget = if path == WORKER_CONTROL_EQUIPMENT_READ_PATH
                        || path == WORKER_CONTROL_SENT_LOOKUP_PATH
                    {
                        REMOTE_READ_REQUEST_TIMEOUT
                    } else {
                        request_timeout
                    };
                    match tokio::time::timeout(budget, next.run(request)).await {
                        Ok(http_response) => http_response,
                        Err(error) => {
                            diagnostics::warn(
                                "control_server",
                                "request_timeout",
                                diagnostics::timeout_kind(&error),
                                DiagnosticContext::default(),
                            );
                            response(
                                StatusCode::GATEWAY_TIMEOUT,
                                ResponseCode::Unavailable,
                                "Worker 控制请求超时",
                                Option::<()>::None,
                                &Uuid::new_v4().to_string(),
                            )
                        }
                    }
                },
            ))
            .layer(Extension(metrics_state))
            .layer(Extension(remote_reads))
            .with_state(handle);
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(WorkerControlServerError::Serve);
        drop(socket_guard);
        drop(lock);
        result
    }
}
