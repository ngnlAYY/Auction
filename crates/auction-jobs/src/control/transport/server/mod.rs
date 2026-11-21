//! Worker control Unix socket 服务端装配：在装配根已安全绑定的 listener 上叠加 peer 凭据过滤与
//! 控制路由。socket 文件、目录链、实例锁与退出清理由该 listener 拥有，这里不会启动第二个监听器。

use std::fmt::Debug;
use std::sync::Arc;

use auction_application::admin::AdminCredentialCheckPort;
use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::operations::{
    WORKER_CONTROL_EQUIPMENT_READ_PATH, WORKER_CONTROL_EXTERNAL_ACCOUNTS_PATH,
    WORKER_CONTROL_LIVE_PATH, WORKER_CONTROL_MAIL_CREDENTIAL_CHECK_PATH,
    WORKER_CONTROL_MAIL_PROGRESS_PATH, WORKER_CONTROL_READY_PATH,
    WORKER_CONTROL_RECOVERY_SCAN_PATH, WORKER_CONTROL_SENT_LOOKUP_PATH, WORKER_CONTROL_STATUS_PATH,
    WORKER_CONTROL_TRIGGER_POLL_PATH, WorkerRemoteReadPort,
};
use auction_observability::ProcessMetrics;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Extension};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::serve::Listener;
use uuid::Uuid;

use super::handlers::credential_check::mail_credential_check;
use super::handlers::equipment_read;
use super::handlers::external_accounts;
use super::handlers::live;
use super::handlers::mail_progress;
use super::handlers::metrics;
use super::handlers::not_found;
use super::handlers::ready;
use super::handlers::recovery_scan;
use super::handlers::sent_lookup;
use super::handlers::status;
use super::handlers::trigger_poll;
use super::protocol::ResponseCode;
use super::protocol::response;
use super::{
    MAIL_CREDENTIAL_CHECK_REQUEST_TIMEOUT, RECOVERY_SCAN_REQUEST_TIMEOUT,
    REMOTE_READ_REQUEST_TIMEOUT, WORKER_CONTROL_METRICS_PATH, WorkerControlHandle,
    WorkerControlServerConfig, WorkerControlServerError,
};
use crate::worker::diagnostics;

mod listener;

use listener::PeerCheckedUnixListener;

/// 只监听本地 Unix socket 的 Worker control server。
pub struct WorkerControlServer<L> {
    listener: PeerCheckedUnixListener<L>,
    handle: WorkerControlHandle,
    config: WorkerControlServerConfig,
    metrics: Option<Arc<ProcessMetrics>>,
    /// 远端只读执行器；未装配时对应路由明确返回能力不可用。
    remote_reads: Option<Arc<dyn WorkerRemoteReadPort>>,
    /// Admin 邮件凭据在线检查的执行器；未装配时路由明确返回能力不可用。
    mail_credential_checks: Option<Arc<dyn AdminCredentialCheckPort>>,
}

impl<L> WorkerControlServer<L>
where
    L: Listener<Io = tokio::net::UnixStream>,
    L::Addr: Debug,
{
    /// 在装配根按 `config` 安全绑定的 Unix listener 上构造 control server，并可选挂载进程本地
    /// Prometheus registry。listener 拥有 socket 文件、实例锁与按身份清理，未进入 `run` 就
    /// Drop 时也由它收口；指标路由复用同一个 peer-checked listener，未注入或关闭指标时仍
    /// fail closed 为 404。
    pub fn new(
        listener: L,
        config: WorkerControlServerConfig,
        handle: WorkerControlHandle,
        metrics: Option<Arc<ProcessMetrics>>,
    ) -> Self {
        let allowed_peer_uids = config.allowed_peer_uids.iter().copied().collect();
        let allowed_peer_gids = config.allowed_peer_gids.iter().copied().collect();
        Self {
            listener: PeerCheckedUnixListener { listener, allowed_peer_uids, allowed_peer_gids },
            handle,
            config,
            metrics,
            remote_reads: None,
            mail_credential_checks: None,
        }
    }

    /// 注入远端只读执行器；Admin 装备读取与 CLI Sent 核查共用 Worker 的请求门、
    /// 账号熔断与暂停判定，Admin/CLI 不再持有第二个限速器。
    pub fn with_remote_reads(mut self, remote_reads: Arc<dyn WorkerRemoteReadPort>) -> Self {
        self.remote_reads = Some(remote_reads);
        self
    }

    /// 注入 Admin 邮件凭据在线检查的执行器；它必须使用与本进程邮件循环共用请求门、在途锁与
    /// 账号忙熔断的客户端，Admin 不再从自身进程直连外部站点。
    pub fn with_mail_credential_checks(
        mut self,
        checks: Arc<dyn AdminCredentialCheckPort>,
    ) -> Self {
        self.mail_credential_checks = Some(checks);
        self
    }

    /// 运行控制 server，收到 shutdown future 后优雅收口。axum 在停止接受连接时就丢弃 listener，
    /// 因此进入关闭即按身份删除 socket、释放实例锁，在途请求随后继续排空；与 Public/Admin 一致。
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
            mail_credential_checks,
        } = self;
        let request_timeout = config.request_timeout;
        let metrics_state = process_metrics;
        let app = Router::new()
            .route(WORKER_CONTROL_LIVE_PATH, get(live))
            .route(WORKER_CONTROL_READY_PATH, get(ready))
            .route(WORKER_CONTROL_STATUS_PATH, get(status))
            .route(WORKER_CONTROL_MAIL_PROGRESS_PATH, get(mail_progress))
            .route(WORKER_CONTROL_EXTERNAL_ACCOUNTS_PATH, get(external_accounts))
            .route(WORKER_CONTROL_TRIGGER_POLL_PATH, post(trigger_poll))
            .route(WORKER_CONTROL_EQUIPMENT_READ_PATH, post(equipment_read))
            .route(WORKER_CONTROL_SENT_LOOKUP_PATH, post(sent_lookup))
            .route(WORKER_CONTROL_RECOVERY_SCAN_PATH, post(recovery_scan))
            .route(WORKER_CONTROL_MAIL_CREDENTIAL_CHECK_PATH, post(mail_credential_check))
            .route(WORKER_CONTROL_METRICS_PATH, get(metrics))
            .fallback(not_found)
            .layer(DefaultBodyLimit::max(config.body_limit))
            .layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| async move {
                    // 远端读取跨越 Worker 自己的 fetch 预算，使用独立的更长上界。
                    let path = request.uri().path().to_owned();
                    let budget = if path == WORKER_CONTROL_RECOVERY_SCAN_PATH {
                        RECOVERY_SCAN_REQUEST_TIMEOUT
                    } else if path == WORKER_CONTROL_EQUIPMENT_READ_PATH
                        || path == WORKER_CONTROL_SENT_LOOKUP_PATH
                    {
                        REMOTE_READ_REQUEST_TIMEOUT
                    } else if path == WORKER_CONTROL_MAIL_CREDENTIAL_CHECK_PATH {
                        MAIL_CREDENTIAL_CHECK_REQUEST_TIMEOUT
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
            .layer(Extension(mail_credential_checks))
            .with_state(handle);
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(WorkerControlServerError::Serve)
    }
}
