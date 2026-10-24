//! Worker control Unix socket 服务端装配；服务生命周期、peer 凭据过滤和 socket
//! 文件安全由子模块拥有，不会启动第二个监听器。

use super::*;

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
            _lock: lock,
            _socket_guard: SocketCleanupGuard::new(cleanup_path, identity),
        })
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
            .route(WORKER_CONTROL_METRICS_PATH, get(metrics))
            .fallback(not_found)
            .layer(DefaultBodyLimit::max(config.body_limit))
            .layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| async move {
                    match tokio::time::timeout(request_timeout, next.run(request)).await {
                        Ok(http_response) => http_response,
                        Err(_) => response(
                            StatusCode::GATEWAY_TIMEOUT,
                            ResponseCode::Unavailable,
                            "Worker 控制请求超时",
                            Option::<()>::None,
                            &Uuid::new_v4().to_string(),
                        ),
                    }
                },
            ))
            .layer(Extension(metrics_state))
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
