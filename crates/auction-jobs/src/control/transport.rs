//! Worker control 的 Unix socket 协议、服务端和 Admin client。
//!
//! 该模块只暴露存活、就绪、状态和 MoogleMail 唤醒四个窄操作。socket 使用安全 runtime 目录、
//! 单实例锁、inode 清理、固定 group/权限和显式 UID/GID peer allowlist；响应不包含配置、Cookie、
//! 游标或错误 cause。唤醒命令只触发下一轮调度，邮件事实仍必须经过数据库命令账本和入站事务。

use std::{
    collections::VecDeque,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

#[cfg(unix)]
use std::{
    collections::HashSet,
    ffi::CString,
    fs::{File, OpenOptions},
};

use auction_application::{
    ApplicationFuture, WorkerControlCondition, WorkerControlPort, WorkerControlStatus,
    WorkerControlTrigger,
};
use auction_observability::{METRICS_ROUTE, MetricsError, ProcessMetrics};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Extension, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::sync::Notify;
use uuid::Uuid;

/// 当前 Worker control HTTP/JSON 协议版本。
pub const WORKER_CONTROL_PROTOCOL_VERSION: &str = "v1";
/// Worker control liveness 路径。
pub const WORKER_CONTROL_LIVE_PATH: &str = "/v1/health/live";
/// Worker control readiness 路径。
pub const WORKER_CONTROL_READY_PATH: &str = "/v1/health/ready";
/// Worker control 状态路径。
pub const WORKER_CONTROL_STATUS_PATH: &str = "/v1/status";
/// Worker control MoogleMail 唤醒路径。
pub const WORKER_CONTROL_TRIGGER_POLL_PATH: &str = "/v1/mail/polls:trigger";
/// Worker control peer-authenticated Prometheus scrape path。
pub const WORKER_CONTROL_METRICS_PATH: &str = METRICS_ROUTE;

const HEADER_PROTOCOL_VERSION: &str = "x-worker-control-version";
const HEADER_REQUEST_ID: &str = "x-request-id";
const DEFAULT_BODY_LIMIT: usize = 1024;
const MAX_RESPONSE_BYTES: usize = 64 << 10;
const MAX_REMEMBERED_TRIGGER_COMMANDS: usize = 256;

/// Worker control server 错误。
#[derive(Debug, Error)]
pub enum WorkerControlServerError {
    #[error("Worker control socket 路径无效")]
    InvalidSocketPath,
    #[error("Worker control runtime 目录无效")]
    InvalidRuntimeDirectory,
    #[error("Worker control 安全配置无效：{0}")]
    InvalidSecurityConfig(&'static str),
    #[error("Worker control Unix group 无法解析")]
    UnknownGroup,
    #[error("Worker control runtime 目录不满足 owner/group/权限约束")]
    UnsafeRuntimeDirectory,
    #[error("Worker control socket 或实例锁已被活动实例占用")]
    ActiveSocket,
    #[error("Worker control socket 状态无法安全确认")]
    UnverifiableSocket,
    #[error("Worker control socket 已被非 socket 文件占用")]
    SocketPathOccupied,
    #[error("Worker control socket 不受当前平台支持")]
    UnsupportedPlatform,
    #[error("Worker control socket IO 失败：{0}")]
    Io(#[source] std::io::Error),
    #[error("Worker control server 运行失败：{0}")]
    Serve(#[source] std::io::Error),
}

/// Worker control client 的安全错误分类；不保存远端正文。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum WorkerControlClientError {
    #[error("Worker control unavailable")]
    Unavailable,
    #[error("Worker control timeout")]
    Timeout,
    #[error("Worker control incompatible")]
    Incompatible,
    #[error("Worker control rejected")]
    Rejected,
}

/// Worker control server 配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerControlServerConfig {
    /// Unix socket 的规范化绝对路径。
    pub socket_path: PathBuf,
    /// 必须预先存在且由当前进程用户拥有的运行目录。
    pub runtime_directory: PathBuf,
    /// socket 文件应归属的 Unix group 名称。
    pub group: String,
    /// 允许访问控制面的内核认证 UID。
    pub allowed_peer_uids: Vec<u32>,
    /// 允许访问控制面的内核认证 GID。
    pub allowed_peer_gids: Vec<u32>,
    /// 单个请求的最长处理时间。
    pub request_timeout: Duration,
    /// JSON 请求体上限；不能超过协议的保守边界。
    pub body_limit: usize,
}

impl WorkerControlServerConfig {
    /// 建立并校验 Worker control server 配置。
    pub fn new(
        socket_path: impl Into<PathBuf>,
        runtime_directory: impl Into<PathBuf>,
        group: impl Into<String>,
        allowed_peer_uids: Vec<u32>,
        allowed_peer_gids: Vec<u32>,
    ) -> Result<Self, WorkerControlServerError> {
        let socket_path = socket_path.into();
        let runtime_directory = runtime_directory.into();
        let group = group.into();
        validate_socket_path(&socket_path)?;
        validate_runtime_directory_path(&runtime_directory)?;
        if socket_path.parent() != Some(runtime_directory.as_path()) {
            return Err(WorkerControlServerError::InvalidSecurityConfig(
                "socket 必须直接位于 runtime 目录",
            ));
        }
        if !valid_safe_identifier(&group) {
            return Err(WorkerControlServerError::InvalidSecurityConfig("group 必须是安全标识"));
        }
        if allowed_peer_uids.len() + allowed_peer_gids.len() > 64 {
            return Err(WorkerControlServerError::InvalidSecurityConfig(
                "peer allowlist 不能超过 64 项",
            ));
        }
        if duplicate_u32(&allowed_peer_uids) || duplicate_u32(&allowed_peer_gids) {
            return Err(WorkerControlServerError::InvalidSecurityConfig(
                "peer allowlist 不能包含重复 ID",
            ));
        }
        if allowed_peer_uids.is_empty() && allowed_peer_gids.is_empty() {
            return Err(WorkerControlServerError::InvalidSecurityConfig(
                "必须显式配置 peer UID 或 GID allowlist",
            ));
        }
        Ok(Self {
            socket_path,
            runtime_directory,
            group,
            allowed_peer_uids,
            allowed_peer_gids,
            request_timeout: Duration::from_secs(5),
            body_limit: DEFAULT_BODY_LIMIT,
        })
    }
}

/// Worker 进程控制状态的内存 owner。
///
/// 该状态只描述当前进程，不是数据库权威状态；`mail_pollers` 只用于判断唤醒是否被接受。
#[derive(Clone)]
pub struct WorkerControlHandle {
    inner: Arc<WorkerControlInner>,
}

struct WorkerControlInner {
    state: std::sync::atomic::AtomicU8,
    ready: std::sync::atomic::AtomicBool,
    mail_poll_requested: std::sync::atomic::AtomicBool,
    version: String,
    mail_pollers: usize,
    poll_wakeup: Arc<Notify>,
    trigger_history: Mutex<TriggerHistory>,
}

#[derive(Clone, Copy)]
enum WorkerControlState {
    Starting = 0,
    Serving = 1,
    Degraded = 2,
    Draining = 3,
}

impl WorkerControlState {
    fn from_byte(value: u8) -> Self {
        match value {
            1 => Self::Serving,
            2 => Self::Degraded,
            3 => Self::Draining,
            _ => Self::Starting,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Serving => "serving",
            Self::Degraded => "degraded",
            Self::Draining => "draining",
        }
    }
}

struct TriggerHistory {
    order: VecDeque<String>,
    results: std::collections::HashMap<String, TriggerPollResult>,
}

impl WorkerControlHandle {
    /// 建立启动中的 Worker control 状态。
    pub fn new(version: impl Into<String>, mail_pollers: usize) -> Self {
        Self {
            inner: Arc::new(WorkerControlInner {
                state: std::sync::atomic::AtomicU8::new(WorkerControlState::Starting as u8),
                ready: std::sync::atomic::AtomicBool::new(false),
                mail_poll_requested: std::sync::atomic::AtomicBool::new(false),
                version: version.into(),
                mail_pollers,
                poll_wakeup: Arc::new(Notify::new()),
                trigger_history: Mutex::new(TriggerHistory {
                    order: VecDeque::with_capacity(MAX_REMEMBERED_TRIGGER_COMMANDS),
                    results: std::collections::HashMap::with_capacity(
                        MAX_REMEMBERED_TRIGGER_COMMANDS,
                    ),
                }),
            }),
        }
    }

    /// 标记 Worker 已经可以接受控制请求。
    pub fn mark_serving(&self) {
        self.inner.ready.store(true, std::sync::atomic::Ordering::Release);
        self.inner
            .state
            .store(WorkerControlState::Serving as u8, std::sync::atomic::Ordering::Release);
    }

    /// 标记 Worker 存活但当前无法接受新工作。
    pub fn mark_degraded(&self) {
        self.inner.ready.store(false, std::sync::atomic::Ordering::Release);
        self.inner
            .state
            .store(WorkerControlState::Degraded as u8, std::sync::atomic::Ordering::Release);
    }

    /// 标记 Worker 正在收口；之后的手动唤醒会被拒绝。
    pub fn mark_draining(&self) {
        self.inner.ready.store(false, std::sync::atomic::Ordering::Release);
        self.inner
            .state
            .store(WorkerControlState::Draining as u8, std::sync::atomic::Ordering::Release);
    }

    /// 返回下一轮 MoogleMail 轮询使用的唤醒器。
    pub fn poll_wakeup(&self) -> Arc<Notify> {
        Arc::clone(&self.inner.poll_wakeup)
    }

    /// 取出一次手动轮询请求；Worker 主循环据此让所有邮箱忽略下一次调度时间。
    pub fn take_mail_poll_request(&self) -> bool {
        self.inner.mail_poll_requested.swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    fn live(&self) -> LiveResult {
        LiveResult { state: self.state_name().to_owned() }
    }

    fn ready(&self) -> ReadyResult {
        ReadyResult { ready: self.inner.ready.load(std::sync::atomic::Ordering::Acquire) }
    }

    fn status(&self) -> StatusResult {
        StatusResult {
            mail_pollers: self.inner.mail_pollers,
            ready: self.inner.ready.load(std::sync::atomic::Ordering::Acquire),
            state: self.state_name().to_owned(),
            version: self.inner.version.clone(),
        }
    }

    fn state_name(&self) -> &'static str {
        WorkerControlState::from_byte(self.inner.state.load(std::sync::atomic::Ordering::Acquire))
            .as_str()
    }

    fn trigger_poll(&self, command: TriggerPollCommand) -> Result<TriggerPollResult, TriggerError> {
        if command.command_id.is_empty()
            || command.command_id.len() > 64
            || command.command_id != command.command_id.trim()
            || command.command_id.chars().any(char::is_control)
            || Uuid::parse_str(&command.command_id).is_err()
        {
            return Err(TriggerError::InvalidCommand);
        }
        let mut history =
            self.inner.trigger_history.lock().map_err(|_| TriggerError::Unavailable)?;
        if let Some(result) = history.results.get(&command.command_id).cloned() {
            return Ok(result);
        }
        if !self.inner.ready.load(std::sync::atomic::Ordering::Acquire)
            || !matches!(
                WorkerControlState::from_byte(
                    self.inner.state.load(std::sync::atomic::Ordering::Acquire),
                ),
                WorkerControlState::Serving
            )
        {
            return Err(TriggerError::NotReady);
        }
        if self.inner.mail_pollers == 0 {
            return Err(TriggerError::NoPollers);
        }
        let result = TriggerPollResult {
            accepted: true,
            command_id: command.command_id.clone(),
            pollers: self.inner.mail_pollers,
        };
        if history.order.len() == MAX_REMEMBERED_TRIGGER_COMMANDS
            && let Some(oldest) = history.order.pop_front()
        {
            history.results.remove(&oldest);
        }
        history.order.push_back(command.command_id);
        history.results.insert(result.command_id.clone(), result.clone());
        drop(history);
        self.inner.mail_poll_requested.store(true, std::sync::atomic::Ordering::Release);
        self.inner.poll_wakeup.notify_one();
        Ok(result)
    }
}

/// Worker liveness 响应数据。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveResult {
    pub state: String,
}

/// Worker readiness 响应数据。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReadyResult {
    pub ready: bool,
}

/// Worker 状态响应数据；不包含配置、凭据或错误正文。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatusResult {
    pub mail_pollers: usize,
    pub ready: bool,
    pub state: String,
    pub version: String,
}

/// Worker 手动轮询请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerPollCommand {
    pub command_id: String,
}

/// Worker 手动轮询响应。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerPollResult {
    pub accepted: bool,
    pub command_id: String,
    pub pollers: usize,
}

#[derive(Debug, Clone, Copy, Error)]
enum TriggerError {
    #[error("invalid trigger command")]
    InvalidCommand,
    #[error("worker is not ready")]
    NotReady,
    #[error("worker has no MoogleMail poller")]
    NoPollers,
    #[error("worker control state unavailable")]
    Unavailable,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ControlEnvelope<T> {
    code: String,
    message: String,
    data: Option<T>,
}

#[derive(Debug, Clone, Copy)]
enum ResponseCode {
    Ok,
    Accepted,
    NotReady,
    Unavailable,
    Invalid,
}

impl ResponseCode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Accepted => "accepted",
            Self::NotReady => "not_ready",
            Self::Unavailable => "unavailable",
            Self::Invalid => "invalid_command_id",
        }
    }
}

fn response<T: Serialize>(
    status: StatusCode,
    code: ResponseCode,
    message: &'static str,
    data: Option<T>,
    request_id: &str,
) -> Response {
    let mut response = (
        status,
        [
            (HEADER_PROTOCOL_VERSION, WORKER_CONTROL_PROTOCOL_VERSION),
            (HEADER_REQUEST_ID, request_id),
        ],
        Json(ControlEnvelope { code: code.as_str().to_owned(), message: message.to_owned(), data }),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

fn request_id(headers: &HeaderMap) -> Result<String, ()> {
    let values = headers.get_all(HEADER_REQUEST_ID);
    let mut iter = values.iter();
    let Some(value) = iter.next() else {
        return Ok(Uuid::new_v4().to_string());
    };
    if iter.next().is_some() {
        return Err(());
    }
    let Ok(value) = value.to_str() else {
        return Err(());
    };
    if value.is_empty()
        || value.len() > 64
        || value != value.trim()
        || value.chars().any(char::is_control)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(());
    }
    Ok(value.to_owned())
}

fn invalid_request_id_response() -> Response {
    response(
        StatusCode::BAD_REQUEST,
        ResponseCode::Invalid,
        "请求标识无效",
        Option::<()>::None,
        &Uuid::new_v4().to_string(),
    )
}

async fn live(State(handle): State<WorkerControlHandle>, headers: HeaderMap) -> Response {
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
    };
    response(StatusCode::OK, ResponseCode::Ok, "Worker 存活", Some(handle.live()), &request_id)
}

async fn ready(State(handle): State<WorkerControlHandle>, headers: HeaderMap) -> Response {
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
    };
    let result = handle.ready();
    if !result.ready {
        return response(
            StatusCode::SERVICE_UNAVAILABLE,
            ResponseCode::NotReady,
            "Worker 尚未就绪",
            Option::<ReadyResult>::None,
            &request_id,
        );
    }
    response(StatusCode::OK, ResponseCode::Ok, "Worker 已就绪", Some(result), &request_id)
}

async fn status(State(handle): State<WorkerControlHandle>, headers: HeaderMap) -> Response {
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
    };
    response(
        StatusCode::OK,
        ResponseCode::Ok,
        "Worker 状态可用",
        Some(handle.status()),
        &request_id,
    )
}

async fn trigger_poll(
    State(handle): State<WorkerControlHandle>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request_id = match request_id(&headers) {
        Ok(request_id) => request_id,
        Err(()) => return invalid_request_id_response(),
    };
    if !headers.get(header::CONTENT_TYPE).is_some_and(|value| {
        value.to_str().ok().is_some_and(|value| {
            value.split(';').next().is_some_and(|value| value.trim() == "application/json")
        })
    }) {
        return response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ResponseCode::Invalid,
            "请求必须使用 application/json",
            Option::<()>::None,
            &request_id,
        );
    }
    let command: TriggerPollCommand = match serde_json::from_slice(&body) {
        Ok(command) => command,
        Err(_) => {
            return response(
                StatusCode::BAD_REQUEST,
                ResponseCode::Invalid,
                "请求 JSON 无效",
                Option::<()>::None,
                &request_id,
            );
        }
    };
    match handle.trigger_poll(command) {
        Ok(result) => response(
            StatusCode::ACCEPTED,
            ResponseCode::Accepted,
            "轮询唤醒已接受",
            Some(result),
            &request_id,
        ),
        Err(TriggerError::InvalidCommand) => response(
            StatusCode::BAD_REQUEST,
            ResponseCode::Invalid,
            "命令标识无效",
            Option::<()>::None,
            &request_id,
        ),
        Err(TriggerError::NotReady | TriggerError::NoPollers) => response(
            StatusCode::SERVICE_UNAVAILABLE,
            ResponseCode::NotReady,
            "Worker 尚未就绪",
            Option::<()>::None,
            &request_id,
        ),
        Err(TriggerError::Unavailable) => response(
            StatusCode::SERVICE_UNAVAILABLE,
            ResponseCode::Unavailable,
            "Worker 控制面不可用",
            Option::<()>::None,
            &request_id,
        ),
    }
}

async fn metrics(Extension(metrics): Extension<Option<Arc<ProcessMetrics>>>, uri: Uri) -> Response {
    if uri.path() != WORKER_CONTROL_METRICS_PATH || uri.query().is_some() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(metrics) = metrics else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match metrics.encode() {
        Ok(body) => {
            ([(header::CONTENT_TYPE, ProcessMetrics::content_type())], body).into_response()
        }
        Err(MetricsError::Disabled) => StatusCode::NOT_FOUND.into_response(),
        Err(MetricsError::Registry(_) | MetricsError::Encode(_)) => {
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn not_found(headers: HeaderMap) -> Response {
    let request_id = request_id(&headers).unwrap_or_else(|_| Uuid::new_v4().to_string());
    response(
        StatusCode::NOT_FOUND,
        ResponseCode::Invalid,
        "请求的资源不存在",
        Option::<()>::None,
        &request_id,
    )
}

/// 只监听本地 Unix socket 的 Worker control server。
#[cfg(unix)]
pub struct WorkerControlServer {
    listener: PeerCheckedUnixListener,
    handle: WorkerControlHandle,
    config: WorkerControlServerConfig,
    metrics: Option<Arc<ProcessMetrics>>,
    _lock: File,
    _socket_guard: SocketCleanupGuard,
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
    ///
    /// 指标路由复用同一个 peer-checked Unix listener；未注入或关闭指标时仍 fail closed 为
    /// 404，不会启动第二个 HTTP 监听器。
    pub async fn bind_with_metrics(
        config: WorkerControlServerConfig,
        handle: WorkerControlHandle,
        metrics: Option<Arc<ProcessMetrics>>,
    ) -> Result<Self, WorkerControlServerError> {
        let group_id = resolve_group_id(&config.group)?;
        validate_runtime_directory(&config.runtime_directory, group_id)?;
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
            _socket_guard: SocketCleanupGuard { path: cleanup_path, identity },
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

#[cfg(unix)]
struct PeerCheckedUnixListener {
    listener: tokio::net::UnixListener,
    allowed_peer_uids: HashSet<u32>,
    allowed_peer_gids: HashSet<u32>,
}

#[cfg(unix)]
impl axum::serve::Listener for PeerCheckedUnixListener {
    type Io = tokio::net::UnixStream;
    type Addr = tokio::net::unix::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.listener.accept().await {
                Ok((stream, address)) => match stream.peer_cred() {
                    Ok(credential)
                        if self.allowed_peer_uids.contains(&credential.uid())
                            || self.allowed_peer_gids.contains(&credential.gid()) =>
                    {
                        return (stream, address);
                    }
                    Ok(credential) => {
                        tracing::warn!(
                            peer_uid = credential.uid(),
                            peer_gid = credential.gid(),
                            peer_pid = ?credential.pid(),
                            "Worker control 拒绝不在显式 UID/GID allowlist 内的 peer"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "Worker control 无法读取 peer credential");
                    }
                },
                Err(error) => {
                    tracing::error!(error = %error, "Worker control accept 失败");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(unix)]
struct SocketCleanupGuard {
    path: PathBuf,
    identity: FileIdentity,
}

#[cfg(unix)]
impl Drop for SocketCleanupGuard {
    fn drop(&mut self) {
        let _ = remove_if_same_socket(&self.path, Some(self.identity));
    }
}

#[cfg(unix)]
fn resolve_group_id(group: &str) -> Result<u32, WorkerControlServerError> {
    let encoded = CString::new(group)
        .map_err(|_| WorkerControlServerError::InvalidSecurityConfig("group 包含 NUL"))?;
    // SAFETY: encoded 是 NUL 结尾的不可变 group 名称；返回记录只在本次同步调用内读取。
    let record = unsafe { libc::getgrnam(encoded.as_ptr()) };
    if record.is_null() {
        return Err(WorkerControlServerError::UnknownGroup);
    }
    // SAFETY: 上一步已排除空指针，gr_gid 是 libc group 记录中的值字段。
    Ok(unsafe { (*record).gr_gid })
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: geteuid 不接收指针，也不访问 Rust 管理的内存。
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn validate_runtime_directory(path: &Path, group_id: u32) -> Result<(), WorkerControlServerError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let resolved = std::fs::canonicalize(path).map_err(WorkerControlServerError::Io)?;
    if resolved != path {
        return Err(WorkerControlServerError::UnsafeRuntimeDirectory);
    }
    let metadata = std::fs::symlink_metadata(path).map_err(WorkerControlServerError::Io)?;
    let mode = metadata.permissions().mode() & 0o777;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != current_uid()
        || mode & 0o002 != 0
        || mode & 0o020 != 0 && metadata.gid() != group_id
    {
        return Err(WorkerControlServerError::UnsafeRuntimeDirectory);
    }
    Ok(())
}

#[cfg(unix)]
fn acquire_instance_lock(socket_path: &Path) -> Result<File, WorkerControlServerError> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let mut lock_name = socket_path.as_os_str().as_bytes().to_vec();
    lock_name.extend_from_slice(b".lock");
    let lock_path = PathBuf::from(std::ffi::OsString::from_vec(lock_name));
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&lock_path)
        .map_err(WorkerControlServerError::Io)?;
    let metadata = lock.metadata().map_err(WorkerControlServerError::Io)?;
    let path_metadata =
        std::fs::symlink_metadata(&lock_path).map_err(WorkerControlServerError::Io)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o777 != 0o600
        || path_metadata.file_type().is_symlink()
        || path_metadata.dev() != metadata.dev()
        || path_metadata.ino() != metadata.ino()
    {
        return Err(WorkerControlServerError::InvalidSecurityConfig(
            "实例锁必须是当前用户拥有的 0600 单链接普通文件",
        ));
    }
    // SAFETY: fd 来自仍由 lock 持有的普通文件；flock 不接管 fd，也不修改 Rust 内存。
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        let raw_error = error.raw_os_error();
        if raw_error == Some(libc::EWOULDBLOCK) || raw_error == Some(libc::EAGAIN) {
            return Err(WorkerControlServerError::ActiveSocket);
        }
        return Err(WorkerControlServerError::Io(error));
    }
    Ok(lock)
}

#[cfg(unix)]
async fn prepare_socket_path(path: &Path) -> Result<(), WorkerControlServerError> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(WorkerControlServerError::Io(error)),
    };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != current_uid()
    {
        return Err(WorkerControlServerError::SocketPathOccupied);
    }
    let identity = FileIdentity { device: metadata.dev(), inode: metadata.ino() };
    match tokio::time::timeout(Duration::from_millis(100), tokio::net::UnixStream::connect(path))
        .await
    {
        Ok(Ok(stream)) => {
            drop(stream);
            Err(WorkerControlServerError::ActiveSocket)
        }
        Ok(Err(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            ) =>
        {
            remove_if_same_socket(path, Some(identity))?;
            Ok(())
        }
        Ok(Err(_)) | Err(_) => Err(WorkerControlServerError::UnverifiableSocket),
    }
}

#[cfg(unix)]
fn secure_bound_socket(
    path: &Path,
    group_id: u32,
    identity: FileIdentity,
) -> Result<(), WorkerControlServerError> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

    let encoded = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| WorkerControlServerError::InvalidSocketPath)?;
    // SAFETY: encoded 是已校验路径的 NUL 结尾副本，uid/gid 均为本地内核标识。
    if unsafe { libc::chown(encoded.as_ptr(), current_uid(), group_id) } != 0 {
        return Err(WorkerControlServerError::Io(std::io::Error::last_os_error()));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))
        .map_err(WorkerControlServerError::Io)?;
    let metadata = std::fs::symlink_metadata(path).map_err(WorkerControlServerError::Io)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != current_uid()
        || metadata.gid() != group_id
        || metadata.permissions().mode() & 0o777 != 0o660
        || (FileIdentity { device: metadata.dev(), inode: metadata.ino() }) != identity
    {
        return Err(WorkerControlServerError::InvalidSecurityConfig(
            "绑定后的 socket owner/group/权限或身份不符合合同",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn socket_identity(path: &Path) -> Result<FileIdentity, WorkerControlServerError> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata = std::fs::symlink_metadata(path).map_err(WorkerControlServerError::Io)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != current_uid()
    {
        return Err(WorkerControlServerError::SocketPathOccupied);
    }
    Ok(FileIdentity { device: metadata.dev(), inode: metadata.ino() })
}

#[cfg(unix)]
fn remove_if_same_socket(
    path: &Path,
    expected: Option<FileIdentity>,
) -> Result<(), WorkerControlServerError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    match socket_identity(path) {
        Ok(current) if current == expected => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(WorkerControlServerError::Io(error)),
        },
        Ok(_) | Err(WorkerControlServerError::SocketPathOccupied) => Ok(()),
        Err(WorkerControlServerError::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Admin 进程使用的 UDS-only Worker control client。
#[derive(Clone)]
pub struct WorkerControlClient {
    client: reqwest::Client,
}

impl WorkerControlClient {
    /// 创建 client；不会在构造期间连接 socket。
    pub fn new(
        socket_path: impl AsRef<Path>,
        request_timeout: Duration,
    ) -> Result<Self, WorkerControlServerError> {
        let socket_path = socket_path.as_ref().to_owned();
        validate_socket_path(&socket_path)?;
        let client = reqwest::Client::builder()
            .unix_socket(socket_path)
            .timeout(request_timeout)
            .redirect(Policy::none())
            .build()
            .map_err(|error| WorkerControlServerError::Io(std::io::Error::other(error)))?;
        Ok(Self { client })
    }

    /// 读取 Worker control 状态；错误只保留稳定分类。
    pub async fn fetch_status(&self) -> Result<StatusResult, WorkerControlClientError> {
        self.request(WORKER_CONTROL_STATUS_PATH, None::<()>, "ok").await
    }

    /// 触发 Worker 唤醒所有已装配的 MoogleMail poller。
    pub async fn trigger_poll(
        &self,
        command_id: Uuid,
    ) -> Result<TriggerPollResult, WorkerControlClientError> {
        let input = TriggerPollCommand { command_id: command_id.to_string() };
        self.request(WORKER_CONTROL_TRIGGER_POLL_PATH, Some(input), "accepted").await
    }

    async fn request<T, I>(
        &self,
        path: &str,
        input: Option<I>,
        expected_code: &str,
    ) -> Result<T, WorkerControlClientError>
    where
        T: DeserializeOwned,
        I: Serialize,
    {
        let request_id = Uuid::new_v4().to_string();
        let url = format!("http://worker-control{path}");
        let mut request = match input {
            Some(input) => self.client.post(url).json(&input),
            None => self.client.get(url),
        };
        request = request
            .header(HEADER_REQUEST_ID, &request_id)
            .header(HEADER_PROTOCOL_VERSION, WORKER_CONTROL_PROTOCOL_VERSION);
        let response = request.send().await.map_err(classify_request_error)?;
        let status = response.status();
        if status == StatusCode::GATEWAY_TIMEOUT {
            return Err(WorkerControlClientError::Timeout);
        }
        if status == StatusCode::SERVICE_UNAVAILABLE {
            return Err(WorkerControlClientError::Unavailable);
        }
        if !status.is_success() {
            return Err(WorkerControlClientError::Rejected);
        }
        let protocol_version =
            response.headers().get(HEADER_PROTOCOL_VERSION).and_then(|value| value.to_str().ok());
        let response_request_id =
            response.headers().get(HEADER_REQUEST_ID).and_then(|value| value.to_str().ok());
        if protocol_version != Some(WORKER_CONTROL_PROTOCOL_VERSION)
            || response_request_id != Some(request_id.as_str())
            || !response.headers().get(header::CONTENT_TYPE).is_some_and(|value| {
                value.to_str().ok().is_some_and(|value| {
                    value.split(';').next().is_some_and(|value| value.trim() == "application/json")
                })
            })
        {
            return Err(WorkerControlClientError::Incompatible);
        }
        if response.content_length().is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) {
            return Err(WorkerControlClientError::Incompatible);
        }
        let body = response.bytes().await.map_err(classify_request_error)?;
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(WorkerControlClientError::Incompatible);
        }
        let envelope: ControlEnvelope<T> =
            serde_json::from_slice(&body).map_err(|_| WorkerControlClientError::Incompatible)?;
        if envelope.code != expected_code || envelope.message.is_empty() {
            return Err(WorkerControlClientError::Incompatible);
        }
        envelope.data.ok_or(WorkerControlClientError::Incompatible)
    }
}

impl WorkerControlPort for WorkerControlClient {
    fn status(&self) -> ApplicationFuture<'_, WorkerControlStatus> {
        Box::pin(async move {
            match self.fetch_status().await {
                Ok(status) if valid_status(&status) => Ok(WorkerControlStatus {
                    condition: WorkerControlCondition::Available,
                    state: status.state,
                    ready: status.ready,
                    version: status.version,
                    mail_pollers: i64::try_from(status.mail_pollers).unwrap_or(i64::MAX),
                }),
                Ok(_) => Ok(WorkerControlStatus::unavailable(WorkerControlCondition::Incompatible)),
                Err(error) => Ok(WorkerControlStatus::unavailable(match error {
                    WorkerControlClientError::Unavailable => WorkerControlCondition::Unavailable,
                    WorkerControlClientError::Timeout => WorkerControlCondition::Timeout,
                    WorkerControlClientError::Incompatible => WorkerControlCondition::Incompatible,
                    WorkerControlClientError::Rejected => WorkerControlCondition::Rejected,
                })),
            }
        })
    }

    fn trigger_poll(&self, command_id: Uuid) -> ApplicationFuture<'_, WorkerControlTrigger> {
        Box::pin(async move {
            match self.trigger_poll(command_id).await {
                Ok(result)
                    if result.accepted
                        && result.command_id == command_id.to_string()
                        && result.pollers > 0 =>
                {
                    Ok(WorkerControlTrigger {
                        condition: WorkerControlCondition::Available,
                        accepted: true,
                        pollers: i64::try_from(result.pollers).unwrap_or(i64::MAX),
                    })
                }
                Ok(_) => Ok(WorkerControlTrigger {
                    condition: WorkerControlCondition::Incompatible,
                    accepted: false,
                    pollers: 0,
                }),
                Err(error) => Ok(WorkerControlTrigger {
                    condition: match error {
                        WorkerControlClientError::Unavailable => {
                            WorkerControlCondition::Unavailable
                        }
                        WorkerControlClientError::Timeout => WorkerControlCondition::Timeout,
                        WorkerControlClientError::Incompatible => {
                            WorkerControlCondition::Incompatible
                        }
                        WorkerControlClientError::Rejected => WorkerControlCondition::Rejected,
                    },
                    accepted: false,
                    pollers: 0,
                }),
            }
        })
    }
}

fn valid_status(status: &StatusResult) -> bool {
    matches!(status.state.as_str(), "starting" | "serving" | "degraded" | "draining")
        && !status.version.is_empty()
        && status.version.len() <= 64
        && status.version == status.version.trim()
        && status.version.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '+' | '-')
        })
}

fn classify_request_error(error: reqwest::Error) -> WorkerControlClientError {
    if error.is_timeout() {
        WorkerControlClientError::Timeout
    } else {
        WorkerControlClientError::Unavailable
    }
}

fn validate_socket_path(path: &Path) -> Result<(), WorkerControlServerError> {
    if !valid_absolute_path(path) || path.as_os_str().len() > 100 {
        return Err(WorkerControlServerError::InvalidSocketPath);
    }
    Ok(())
}

fn validate_runtime_directory_path(path: &Path) -> Result<(), WorkerControlServerError> {
    if !valid_absolute_path(path) {
        return Err(WorkerControlServerError::InvalidRuntimeDirectory);
    }
    Ok(())
}

fn valid_absolute_path(path: &Path) -> bool {
    let normalized: PathBuf = path.components().collect();
    path.is_absolute()
        && path != Path::new("/")
        && normalized.as_os_str() == path.as_os_str()
        && !path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        && !path.to_string_lossy().chars().any(|character| character == '\0')
}

fn valid_safe_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(first) if first.is_ascii_alphanumeric())
        && value.len() <= 64
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn duplicate_u32(values: &[u32]) -> bool {
    let mut values = values.to_vec();
    values.sort_unstable();
    values.windows(2).any(|pair| pair[0] == pair[1])
}
